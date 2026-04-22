//! gRPC handler for `OidcLoginService` — OIDC authorize + callback flows.
//!
//! Designed around an injectable `OidcExchanger` trait so that the OIDC HTTP
//! operations (discovery + token exchange) can be replaced in unit tests
//! without standing up a real IdP.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use dashmap::DashMap;
use tonic::{Request, Response, Status};
use tracing::instrument;
use uuid::Uuid;

use stitchd_core::{
    auth::{OrgRole, ProviderType, jwt::JwtEngine},
    id::{AuthProviderId, OrganisationId},
};
use stitchd_db::{AuthProviderRepository, AuthUserRepository, OrgMembershipRepository, RefreshTokenRepository, RepositoryError};
use stitchd_proto::auth::v1::{
    OidcAuthorizeRequest, OidcAuthorizeResponse, OidcCallbackRequest, OidcCallbackResponse,
    oidc_authorize_request::Scope,
    oidc_login_service_server::OidcLoginService,
};

// ---------------------------------------------------------------------------
// OidcExchanger — port for OIDC HTTP operations (mockable in tests)
// ---------------------------------------------------------------------------

/// Port for OIDC HTTP operations (authorization URL generation + token exchange).
#[async_trait]
pub trait OidcExchanger: Send + Sync {
    /// Generate an authorization URL for the given provider.
    ///
    /// Returns `(redirect_url, pkce_verifier_secret, csrf_state)`.
    async fn authorize(
        &self,
        provider_id: AuthProviderId,
        redirect_uri: &str,
    ) -> Result<(String, String, String), Status>;

    /// Exchange an authorization code for the authenticated user's email.
    async fn exchange(
        &self,
        provider_id: AuthProviderId,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<String, Status>; // email
}

// ---------------------------------------------------------------------------
// OidcStateStore — in-memory pending-state map with TTL
// ---------------------------------------------------------------------------

struct OidcPendingState {
    provider_id: AuthProviderId,
    pkce_verifier: String,
    expiry: Instant,
}

pub struct OidcStateStore {
    store: DashMap<String, OidcPendingState>,
    ttl: Duration,
}

impl OidcStateStore {
    pub fn new(ttl: Duration) -> Self {
        Self { store: DashMap::new(), ttl }
    }

    fn insert(&self, csrf: String, provider_id: AuthProviderId, verifier: String) {
        self.store.insert(
            csrf,
            OidcPendingState {
                provider_id,
                pkce_verifier: verifier,
                expiry: Instant::now() + self.ttl,
            },
        );
    }

    fn consume(&self, csrf: &str) -> Option<OidcPendingState> {
        self.store.remove(csrf).map(|(_, v)| v)
    }
}

// ---------------------------------------------------------------------------
// Service implementation
// ---------------------------------------------------------------------------

pub struct OidcLoginServiceImpl {
    exchanger: Arc<dyn OidcExchanger>,
    state_store: Arc<OidcStateStore>,
    auth_user_repo: Arc<dyn AuthUserRepository>,
    membership_repo: Arc<dyn OrgMembershipRepository>,
    refresh_token_repo: Arc<dyn RefreshTokenRepository>,
    provider_repo: Arc<dyn AuthProviderRepository>,
}

impl OidcLoginServiceImpl {
    #[must_use]
    pub fn new(
        exchanger: Arc<dyn OidcExchanger>,
        state_store: Arc<OidcStateStore>,
        auth_user_repo: Arc<dyn AuthUserRepository>,
        membership_repo: Arc<dyn OrgMembershipRepository>,
        refresh_token_repo: Arc<dyn RefreshTokenRepository>,
        provider_repo: Arc<dyn AuthProviderRepository>,
    ) -> Self {
        Self {
            exchanger,
            state_store,
            auth_user_repo,
            membership_repo,
            refresh_token_repo,
            provider_repo,
        }
    }
}

fn map_repo_err(e: RepositoryError) -> Status {
    match e {
        RepositoryError::NotFound { id } => Status::not_found(format!("not found: {id}")),
        other => Status::internal(other.to_string()),
    }
}

fn parse_provider_id(s: &str) -> Result<AuthProviderId, Status> {
    Uuid::parse_str(s)
        .map(AuthProviderId::from_uuid)
        .map_err(|_| Status::invalid_argument("provider_id is not a valid UUID"))
}

fn parse_org_id(s: &str) -> Result<OrganisationId, Status> {
    Uuid::parse_str(s)
        .map(OrganisationId::from_uuid)
        .map_err(|_| Status::invalid_argument("org_id is not a valid UUID"))
}

#[tonic::async_trait]
impl OidcLoginService for OidcLoginServiceImpl {
    #[instrument(skip_all)]
    async fn oidc_authorize(
        &self,
        request: Request<OidcAuthorizeRequest>,
    ) -> Result<Response<OidcAuthorizeResponse>, Status> {
        let req = request.into_inner();

        let provider_id = match req.scope {
            Some(Scope::ProviderId(id)) => parse_provider_id(&id)?,
            Some(Scope::OrgId(org_id)) => {
                let org_id = parse_org_id(&org_id)?;
                let providers = self
                    .provider_repo
                    .list_for_org(org_id)
                    .await
                    .map_err(map_repo_err)?;
                providers
                    .into_iter()
                    .find(|p| p.enabled && p.provider_type == ProviderType::Oidc)
                    .ok_or_else(|| Status::not_found("no enabled OIDC provider for org"))?
                    .id
            }
            None => {
                return Err(Status::invalid_argument(
                    "either provider_id or org_id must be set",
                ));
            }
        };

        let (redirect_url, verifier, csrf) =
            self.exchanger.authorize(provider_id, &req.redirect_uri).await?;

        self.state_store.insert(csrf, provider_id, verifier);

        Ok(Response::new(OidcAuthorizeResponse { redirect_url }))
    }

    #[instrument(skip_all)]
    async fn oidc_callback(
        &self,
        request: Request<OidcCallbackRequest>,
    ) -> Result<Response<OidcCallbackResponse>, Status> {
        let req = request.into_inner();

        let pending = self
            .state_store
            .consume(&req.state)
            .ok_or_else(|| Status::unauthenticated("invalid or expired OIDC state"))?;

        if Instant::now() > pending.expiry {
            return Err(Status::unauthenticated("OIDC state expired"));
        }

        let email = self
            .exchanger
            .exchange(pending.provider_id, &req.code, &pending.pkce_verifier, &req.redirect_uri)
            .await?;

        // Look up the provider to get org_id.
        let provider = self
            .provider_repo
            .find_by_id(pending.provider_id)
            .await
            .map_err(map_repo_err)?
            .ok_or_else(|| Status::not_found("auth provider not found"))?;
        let org_id = provider.org_id;

        // Find or create user.
        let user = match self
            .auth_user_repo
            .find_by_email(&email)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
        {
            Some(u) => u,
            None => self
                .auth_user_repo
                .create(&email, &email, None)
                .await
                .map_err(|e| Status::internal(e.to_string()))?,
        };

        // Find or create org membership.
        let membership = match self
            .membership_repo
            .find_membership(user.id, org_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?
        {
            Some(m) => m,
            None => self
                .membership_repo
                .add_member(user.id, org_id, OrgRole::OrgMember)
                .await
                .map_err(|e| Status::internal(e.to_string()))?,
        };

        let access_token = JwtEngine::issue(
            user.id,
            membership.org_id,
            &user.email,
            membership.role,
            false,
            &user.token_secret,
        )
        .map_err(|e| Status::internal(e.to_string()))?;

        let (_, raw_refresh) = self
            .refresh_token_repo
            .create(user.id, membership.org_id, None, 30)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(OidcCallbackResponse {
            access_token,
            refresh_token: raw_refresh,
            expires_in: 3600,
            user_id: user.id.to_string(),
            org_id: membership.org_id.to_string(),
        }))
    }
}

// ---------------------------------------------------------------------------
// Live OidcExchanger — wraps ProviderCaches + OidcProviderFactory
// ---------------------------------------------------------------------------

use crate::{app_state::ProviderCaches, oidc_factory::OidcProviderFactory};

pub struct LiveOidcExchanger {
    caches: Arc<ProviderCaches>,
    factory: Arc<OidcProviderFactory>,
}

impl LiveOidcExchanger {
    #[must_use]
    pub fn new(caches: Arc<ProviderCaches>, factory: Arc<OidcProviderFactory>) -> Self {
        Self { caches, factory }
    }
}

#[async_trait]
impl OidcExchanger for LiveOidcExchanger {
    async fn authorize(
        &self,
        provider_id: AuthProviderId,
        redirect_uri: &str,
    ) -> Result<(String, String, String), Status> {
        let provider = self
            .caches
            .oidc
            .get_or_build(provider_id, || {
                let factory = Arc::clone(&self.factory);
                async move { factory.build(provider_id).await }
            })
            .await
            .map_err(|e| Status::internal(format!("failed to build OIDC provider: {e}")))?;

        let (url, verifier, csrf) = provider
            .authorization_url(redirect_uri)
            .map_err(|e| Status::internal(format!("authorization_url failed: {e}")))?;

        Ok((url.to_string(), verifier, csrf))
    }

    async fn exchange(
        &self,
        provider_id: AuthProviderId,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<String, Status> {
        let provider = self
            .caches
            .oidc
            .get_or_build(provider_id, || {
                let factory = Arc::clone(&self.factory);
                async move { factory.build(provider_id).await }
            })
            .await
            .map_err(|e| Status::internal(format!("failed to build OIDC provider: {e}")))?;

        provider
            .exchange_code(code, verifier, redirect_uri)
            .await
            .map_err(|e| Status::unauthenticated(format!("code exchange failed: {e}")))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::sync::Arc;
    use stitchd_core::{
        auth::{AuthProvider, OrgRole, ProviderType, User, UserStatus},
        id::{AuthProviderId, OrganisationId, UserId},
    };
    use stitchd_db::{AuthProviderRepository, AuthUserRepository, OrgMembershipRepository, RepositoryError};
    use stitchd_db::RefreshTokenRepository;
    use stitchd_core::auth::OrgMembership;
    use stitchd_core::auth::RefreshToken;

    // ── MockOidcExchanger ────────────────────────────────────────────────────

    struct MockOidcExchanger {
        redirect_url: String,
        verifier: String,
        csrf: String,
        email: String,
    }

    #[async_trait]
    impl OidcExchanger for MockOidcExchanger {
        async fn authorize(
            &self,
            _provider_id: AuthProviderId,
            _redirect_uri: &str,
        ) -> Result<(String, String, String), Status> {
            Ok((self.redirect_url.clone(), self.verifier.clone(), self.csrf.clone()))
        }

        async fn exchange(
            &self,
            _provider_id: AuthProviderId,
            _code: &str,
            _verifier: &str,
            _redirect_uri: &str,
        ) -> Result<String, Status> {
            Ok(self.email.clone())
        }
    }

    // ── MockAuthProviderRepo ─────────────────────────────────────────────────

    struct MockAuthProviderRepo {
        provider: Option<AuthProvider>,
        org_providers: Vec<AuthProvider>,
    }

    fn make_oidc_provider(org_id: OrganisationId) -> AuthProvider {
        AuthProvider {
            id: AuthProviderId::new(),
            org_id,
            provider_type: ProviderType::Oidc,
            display_name: "Test OIDC".to_string(),
            config: serde_json::json!({
                "issuer_url": "https://accounts.google.com",
                "client_id": "cid",
                "client_secret_enc": ""
            }),
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[async_trait]
    impl AuthProviderRepository for MockAuthProviderRepo {
        async fn create(
            &self,
            _org_id: OrganisationId,
            _provider_type: ProviderType,
            _display_name: &str,
            _config_encrypted: serde_json::Value,
            _enabled: bool,
        ) -> Result<AuthProvider, RepositoryError> {
            unimplemented!()
        }

        async fn find_by_id(
            &self,
            _id: AuthProviderId,
        ) -> Result<Option<AuthProvider>, RepositoryError> {
            Ok(self.provider.clone())
        }

        async fn list_for_org(
            &self,
            _org_id: OrganisationId,
        ) -> Result<Vec<AuthProvider>, RepositoryError> {
            Ok(self.org_providers.clone())
        }

        async fn update(
            &self,
            _id: AuthProviderId,
            _display_name: &str,
            _config_encrypted: serde_json::Value,
            _enabled: bool,
        ) -> Result<AuthProvider, RepositoryError> {
            unimplemented!()
        }

        async fn delete(&self, _id: AuthProviderId) -> Result<(), RepositoryError> {
            unimplemented!()
        }
    }

    // ── MockAuthUserRepo ─────────────────────────────────────────────────────

    struct MockAuthUserRepo {
        existing_user: Option<User>,
    }

    fn make_user(email: &str) -> User {
        User {
            id: UserId::new(),
            email: email.to_string(),
            display_name: email.to_string(),
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

    #[async_trait]
    impl AuthUserRepository for MockAuthUserRepo {
        async fn create(
            &self,
            email: &str,
            _display_name: &str,
            _password_hash: Option<&str>,
        ) -> Result<User, RepositoryError> {
            Ok(make_user(email))
        }

        async fn find_by_email(&self, _email: &str) -> Result<Option<User>, RepositoryError> {
            Ok(self.existing_user.clone())
        }

        async fn find_by_id(&self, _id: UserId) -> Result<Option<User>, RepositoryError> {
            Ok(self.existing_user.clone())
        }

        async fn rotate_token_secret(
            &self,
            id: UserId,
        ) -> Result<uuid::Uuid, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn update_status(
            &self,
            _id: UserId,
            _status: UserStatus,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn update_password_hash(
            &self,
            _id: UserId,
            _hash: &str,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn update_profile(
            &self,
            _id: UserId,
            _display_name: &str,
            _avatar_url: Option<&str>,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn list_org_users(
            &self,
            _org_id: OrganisationId,
        ) -> Result<Vec<(User, OrgRole)>, RepositoryError> {
            Ok(vec![])
        }
    }

    // ── MockOrgMembershipRepo ────────────────────────────────────────────────

    struct MockOrgMembershipRepo {
        membership: Option<OrgMembership>,
    }

    #[async_trait]
    impl OrgMembershipRepository for MockOrgMembershipRepo {
        async fn add_member(
            &self,
            user_id: UserId,
            org_id: OrganisationId,
            role: OrgRole,
        ) -> Result<OrgMembership, RepositoryError> {
            Ok(OrgMembership { user_id, org_id, role, joined_at: Utc::now() })
        }

        async fn find_membership(
            &self,
            _user_id: UserId,
            _org_id: OrganisationId,
        ) -> Result<Option<OrgMembership>, RepositoryError> {
            Ok(self.membership.clone())
        }

        async fn list_orgs_for_user(
            &self,
            user_id: UserId,
        ) -> Result<Vec<OrgMembership>, RepositoryError> {
            Ok(self.membership.clone().map_or_else(Vec::new, |m| {
                vec![OrgMembership {
                    user_id,
                    org_id: m.org_id,
                    role: m.role,
                    joined_at: m.joined_at,
                }]
            }))
        }

        async fn remove_member(
            &self,
            _user_id: UserId,
            _org_id: OrganisationId,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn update_role(
            &self,
            _user_id: UserId,
            _org_id: OrganisationId,
            _role: OrgRole,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
    }

    // ── MockRefreshTokenRepo ─────────────────────────────────────────────────

    struct MockRefreshTokenRepo;

    #[async_trait]
    impl RefreshTokenRepository for MockRefreshTokenRepo {
        async fn create(
            &self,
            user_id: UserId,
            org_id: OrganisationId,
            _device_hint: Option<&str>,
            _ttl_days: i64,
        ) -> Result<(RefreshToken, String), RepositoryError> {
            use stitchd_core::id::RefreshTokenId;
            let rt = RefreshToken {
                id: RefreshTokenId::new(),
                user_id,
                org_id,
                token_hash: "hash".to_string(),
                device_hint: None,
                issued_at: Utc::now(),
                expires_at: Utc::now(),
                revoked_at: None,
                last_used_at: None,
            };
            Ok((rt, "raw-refresh-token".to_string()))
        }

        async fn find_by_hash(
            &self,
            _hash: &str,
        ) -> Result<Option<RefreshToken>, RepositoryError> {
            Ok(None)
        }

        async fn consume(
            &self,
            _id: stitchd_core::id::RefreshTokenId,
        ) -> Result<Option<RefreshToken>, RepositoryError> {
            Ok(None)
        }

        async fn revoke(
            &self,
            _id: stitchd_core::id::RefreshTokenId,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn revoke_all_for_user(
            &self,
            _user_id: UserId,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn list_active(
            &self,
            _user_id: UserId,
        ) -> Result<Vec<RefreshToken>, RepositoryError> {
            Ok(vec![])
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn make_service(
        exchanger: impl OidcExchanger + 'static,
        provider: Option<AuthProvider>,
        org_providers: Vec<AuthProvider>,
        existing_user: Option<User>,
        membership: Option<OrgMembership>,
    ) -> OidcLoginServiceImpl {
        let org_id = provider
            .as_ref()
            .map(|p| p.org_id)
            .unwrap_or_else(OrganisationId::new);
        let membership = membership.or_else(|| {
            existing_user.as_ref().map(|u| OrgMembership {
                user_id: u.id,
                org_id,
                role: OrgRole::OrgMember,
                joined_at: Utc::now(),
            })
        });
        OidcLoginServiceImpl::new(
            Arc::new(exchanger),
            Arc::new(OidcStateStore::new(Duration::from_secs(300))),
            Arc::new(MockAuthUserRepo { existing_user }),
            Arc::new(MockOrgMembershipRepo { membership }),
            Arc::new(MockRefreshTokenRepo),
            Arc::new(MockAuthProviderRepo { provider, org_providers }),
        )
    }

    fn mock_exchanger() -> MockOidcExchanger {
        MockOidcExchanger {
            redirect_url: "https://accounts.google.com/o/oauth2/auth?client_id=x".to_string(),
            verifier: "pkce-verifier-secret".to_string(),
            csrf: "csrf-state-value".to_string(),
            email: "user@example.com".to_string(),
        }
    }

    // ── Tests ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn authorize_provider_scoped_stores_state_and_returns_redirect_url() {
        let org_id = OrganisationId::new();
        let provider = make_oidc_provider(org_id);
        let provider_id = provider.id;
        let svc = make_service(mock_exchanger(), Some(provider), vec![], None, None);

        let req = tonic::Request::new(OidcAuthorizeRequest {
            scope: Some(Scope::ProviderId(provider_id.to_string())),
            redirect_uri: "https://app.example.com/callback".to_string(),
        });

        let resp = svc.oidc_authorize(req).await;
        assert!(resp.is_ok(), "expected Ok, got: {resp:?}");
        let body = resp.unwrap().into_inner();
        assert!(
            body.redirect_url.contains("accounts.google.com"),
            "redirect_url should be the IdP URL: {}",
            body.redirect_url,
        );

        // State must be stored so callback can consume it.
        assert!(
            svc.state_store.store.contains_key("csrf-state-value"),
            "pending state should be stored keyed on csrf"
        );
    }

    #[tokio::test]
    async fn authorize_org_scoped_picks_first_enabled_oidc_provider() {
        let org_id = OrganisationId::new();
        let provider = make_oidc_provider(org_id);
        let org_providers = vec![provider.clone()];
        let svc = make_service(mock_exchanger(), Some(provider), org_providers, None, None);

        let req = tonic::Request::new(OidcAuthorizeRequest {
            scope: Some(Scope::OrgId(org_id.to_string())),
            redirect_uri: "https://app.example.com/callback".to_string(),
        });

        let resp = svc.oidc_authorize(req).await;
        assert!(resp.is_ok(), "org-scoped authorize should succeed");
        let body = resp.unwrap().into_inner();
        assert!(!body.redirect_url.is_empty(), "redirect_url must be non-empty");
    }

    #[tokio::test]
    async fn authorize_org_scoped_no_oidc_provider_returns_not_found() {
        let org_id = OrganisationId::new();
        let svc = make_service(mock_exchanger(), None, vec![], None, None);

        let req = tonic::Request::new(OidcAuthorizeRequest {
            scope: Some(Scope::OrgId(org_id.to_string())),
            redirect_uri: "https://app.example.com/callback".to_string(),
        });

        let resp = svc.oidc_authorize(req).await;
        assert!(
            matches!(resp, Err(ref s) if s.code() == tonic::Code::NotFound),
            "expected NotFound, got: {resp:?}"
        );
    }

    #[tokio::test]
    async fn callback_valid_state_issues_jwt_and_refresh_token() {
        let org_id = OrganisationId::new();
        let provider = make_oidc_provider(org_id);
        let user = make_user("user@example.com");
        let membership = OrgMembership {
            user_id: user.id,
            org_id,
            role: OrgRole::OrgMember,
            joined_at: Utc::now(),
        };
        let svc = make_service(
            mock_exchanger(),
            Some(provider.clone()),
            vec![],
            Some(user),
            Some(membership),
        );

        // Plant a pending state manually.
        svc.state_store.insert(
            "csrf-state-value".to_string(),
            provider.id,
            "pkce-verifier-secret".to_string(),
        );

        let req = tonic::Request::new(OidcCallbackRequest {
            provider_id: provider.id.to_string(),
            code: "auth-code-from-idp".to_string(),
            state: "csrf-state-value".to_string(),
            redirect_uri: "https://app.example.com/callback".to_string(),
        });

        let resp = svc.oidc_callback(req).await;
        assert!(resp.is_ok(), "expected Ok, got: {resp:?}");
        let body = resp.unwrap().into_inner();
        assert!(!body.access_token.is_empty(), "access_token must be non-empty");
        assert!(!body.refresh_token.is_empty(), "refresh_token must be non-empty");
        assert_eq!(body.expires_in, 3600);
        assert!(!body.user_id.is_empty());
        assert_eq!(body.org_id, org_id.to_string());
    }

    #[tokio::test]
    async fn callback_unknown_state_returns_unauthenticated() {
        let org_id = OrganisationId::new();
        let provider = make_oidc_provider(org_id);
        let svc = make_service(mock_exchanger(), Some(provider), vec![], None, None);

        let req = tonic::Request::new(OidcCallbackRequest {
            provider_id: "".to_string(),
            code: "code".to_string(),
            state: "unknown-state-that-was-never-inserted".to_string(),
            redirect_uri: "https://app.example.com/callback".to_string(),
        });

        let resp = svc.oidc_callback(req).await;
        assert!(
            matches!(resp, Err(ref s) if s.code() == tonic::Code::Unauthenticated),
            "expected Unauthenticated, got: {resp:?}"
        );
    }

    #[tokio::test]
    async fn callback_expired_state_returns_unauthenticated() {
        let org_id = OrganisationId::new();
        let provider = make_oidc_provider(org_id);
        let svc = make_service(mock_exchanger(), Some(provider.clone()), vec![], None, None);

        // Plant a state that has already expired (ttl = 0 nanos ago).
        svc.state_store.store.insert(
            "expired-state".to_string(),
            OidcPendingState {
                provider_id: provider.id,
                pkce_verifier: "verifier".to_string(),
                expiry: Instant::now() - Duration::from_secs(1),
            },
        );

        let req = tonic::Request::new(OidcCallbackRequest {
            provider_id: provider.id.to_string(),
            code: "code".to_string(),
            state: "expired-state".to_string(),
            redirect_uri: "https://app.example.com/callback".to_string(),
        });

        let resp = svc.oidc_callback(req).await;
        assert!(
            matches!(resp, Err(ref s) if s.code() == tonic::Code::Unauthenticated),
            "expected Unauthenticated for expired state, got: {resp:?}"
        );
    }
}
