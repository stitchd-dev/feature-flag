//! gRPC handler for `SamlLoginService` — SAML SSO initiation + ACS callback.
//!
//! Uses an injectable `SamlExchanger` trait (same pattern as `OidcExchanger`)
//! so that SAML XML operations can be mocked in unit tests.

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
use stitchd_db::{
    AuthProviderRepository, AuthUserRepository, OrgMembershipRepository, RefreshTokenRepository,
    RepositoryError,
};
use stitchd_proto::auth::v1::{
    SamlAcsRequest, SamlAcsResponse, SamlSsoRequest, SamlSsoResponse,
    saml_login_service_server::SamlLoginService, saml_sso_request::Scope,
};

// ---------------------------------------------------------------------------
// SamlExchanger — port for SAML SP operations (mockable in tests)
// ---------------------------------------------------------------------------

/// Port for SAML SP operations — SSO initiation and ACS response validation.
#[async_trait]
pub trait SamlExchanger: Send + Sync {
    /// Build the `IdP` redirect URL for SSO initiation.
    ///
    /// Returns `(redirect_url, relay_state)`.
    async fn initiate_sso(
        &self,
        provider_id: AuthProviderId,
        acs_url: &str,
        relay_state: &str,
    ) -> Result<String, Status>; // redirect_url

    /// Validate a base64-encoded `SAMLResponse` and return the user's email.
    async fn validate_response(
        &self,
        provider_id: AuthProviderId,
        saml_response_b64: &str,
    ) -> Result<String, Status>; // email
}

// ---------------------------------------------------------------------------
// SamlRelayStore — in-memory pending-state map with TTL
// ---------------------------------------------------------------------------

struct SamlPendingState {
    provider_id: AuthProviderId,
    expiry: Instant,
}

pub struct SamlRelayStore {
    store: DashMap<String, SamlPendingState>,
    ttl: Duration,
}

impl SamlRelayStore {
    #[must_use]
    pub fn new(ttl: Duration) -> Self {
        Self {
            store: DashMap::new(),
            ttl,
        }
    }

    pub fn insert(&self, relay_state: String, provider_id: AuthProviderId) {
        self.store.insert(
            relay_state,
            SamlPendingState {
                provider_id,
                expiry: Instant::now() + self.ttl,
            },
        );
    }

    fn consume(&self, relay_state: &str) -> Option<SamlPendingState> {
        self.store.remove(relay_state).map(|(_, v)| v)
    }
}

// ---------------------------------------------------------------------------
// Service implementation
// ---------------------------------------------------------------------------

pub struct SamlLoginServiceImpl {
    exchanger: Arc<dyn SamlExchanger>,
    relay_store: Arc<SamlRelayStore>,
    auth_user_repo: Arc<dyn AuthUserRepository>,
    membership_repo: Arc<dyn OrgMembershipRepository>,
    refresh_token_repo: Arc<dyn RefreshTokenRepository>,
    provider_repo: Arc<dyn AuthProviderRepository>,
}

impl SamlLoginServiceImpl {
    #[must_use]
    pub fn new(
        exchanger: Arc<dyn SamlExchanger>,
        relay_store: Arc<SamlRelayStore>,
        auth_user_repo: Arc<dyn AuthUserRepository>,
        membership_repo: Arc<dyn OrgMembershipRepository>,
        refresh_token_repo: Arc<dyn RefreshTokenRepository>,
        provider_repo: Arc<dyn AuthProviderRepository>,
    ) -> Self {
        Self {
            exchanger,
            relay_store,
            auth_user_repo,
            membership_repo,
            refresh_token_repo,
            provider_repo,
        }
    }

    /// Access the relay store — used in integration tests to pre-plant state.
    #[must_use]
    pub const fn relay_store(&self) -> &Arc<SamlRelayStore> {
        &self.relay_store
    }

    fn sp_base_url() -> String {
        std::env::var("SP_BASE_URL").unwrap_or_else(|_| "http://localhost:8080".to_string())
    }

    fn acs_url(provider_id: AuthProviderId) -> String {
        format!(
            "{}/v1/auth/saml/{}/callback",
            Self::sp_base_url(),
            provider_id
        )
    }
}

fn map_repo_err(e: RepositoryError) -> Status {
    match e {
        RepositoryError::NotFound { id } => Status::not_found(format!("not found: {id}")),
        other => Status::internal(other.to_string()),
    }
}

#[allow(clippy::result_large_err)]
fn parse_provider_id(s: &str) -> Result<AuthProviderId, Status> {
    Uuid::parse_str(s)
        .map(AuthProviderId::from_uuid)
        .map_err(|_| Status::invalid_argument("provider_id is not a valid UUID"))
}

#[allow(clippy::result_large_err)]
fn parse_org_id(s: &str) -> Result<OrganisationId, Status> {
    Uuid::parse_str(s)
        .map(OrganisationId::from_uuid)
        .map_err(|_| Status::invalid_argument("org_id is not a valid UUID"))
}

#[tonic::async_trait]
impl SamlLoginService for SamlLoginServiceImpl {
    #[instrument(skip_all)]
    async fn saml_sso_initiate(
        &self,
        request: Request<SamlSsoRequest>,
    ) -> Result<Response<SamlSsoResponse>, Status> {
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
                    .find(|p| p.enabled && p.provider_type == ProviderType::Saml)
                    .ok_or_else(|| Status::not_found("no enabled SAML provider for org"))?
                    .id
            }
            None => {
                return Err(Status::invalid_argument(
                    "either provider_id or org_id must be set",
                ));
            }
        };

        let acs_url = req.acs_url.if_empty_use(|| Self::acs_url(provider_id));
        let relay_state = Uuid::new_v4().to_string();

        let redirect_url = self
            .exchanger
            .initiate_sso(provider_id, &acs_url, &relay_state)
            .await?;

        self.relay_store.insert(relay_state.clone(), provider_id);

        Ok(Response::new(SamlSsoResponse {
            redirect_url,
            relay_state,
        }))
    }

    #[instrument(skip_all)]
    async fn saml_acs_callback(
        &self,
        request: Request<SamlAcsRequest>,
    ) -> Result<Response<SamlAcsResponse>, Status> {
        let req = request.into_inner();

        let pending = self
            .relay_store
            .consume(&req.relay_state)
            .ok_or_else(|| Status::unauthenticated("invalid or replayed SAML RelayState"))?;

        if Instant::now() > pending.expiry {
            return Err(Status::unauthenticated("SAML RelayState expired"));
        }

        let email = self
            .exchanger
            .validate_response(pending.provider_id, &req.saml_response_b64)
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

        Ok(Response::new(SamlAcsResponse {
            access_token,
            refresh_token: raw_refresh,
            expires_in: 3600,
            user_id: user.id.to_string(),
            org_id: membership.org_id.to_string(),
        }))
    }
}

// ---------------------------------------------------------------------------
// Helper — empty-string fallback
// ---------------------------------------------------------------------------

trait IfEmptyUse {
    fn if_empty_use(self, f: impl FnOnce() -> Self) -> Self;
}

impl IfEmptyUse for String {
    fn if_empty_use(self, f: impl FnOnce() -> Self) -> Self {
        if self.is_empty() { f() } else { self }
    }
}

// ---------------------------------------------------------------------------
// Live SamlExchanger — wraps ProviderCaches + SamlProviderFactory
// ---------------------------------------------------------------------------

use crate::{app_state::ProviderCaches, saml_factory::SamlProviderFactory};

pub struct LiveSamlExchanger {
    caches: Arc<ProviderCaches>,
    factory: Arc<SamlProviderFactory>,
}

impl LiveSamlExchanger {
    #[must_use]
    pub const fn new(caches: Arc<ProviderCaches>, factory: Arc<SamlProviderFactory>) -> Self {
        Self { caches, factory }
    }
}

#[async_trait]
impl SamlExchanger for LiveSamlExchanger {
    async fn initiate_sso(
        &self,
        provider_id: AuthProviderId,
        acs_url: &str,
        relay_state: &str,
    ) -> Result<String, Status> {
        let provider = self
            .caches
            .saml
            .get_or_build(provider_id, || {
                let factory = Arc::clone(&self.factory);
                async move { factory.build(provider_id).await }
            })
            .await
            .map_err(|e| Status::internal(format!("failed to build SAML provider: {e}")))?;

        let (_, redirect_url) = provider
            .authn_request(acs_url, relay_state)
            .map_err(|e| Status::internal(format!("authn_request failed: {e}")))?;

        Ok(redirect_url)
    }

    async fn validate_response(
        &self,
        provider_id: AuthProviderId,
        saml_response_b64: &str,
    ) -> Result<String, Status> {
        let provider = self
            .caches
            .saml
            .get_or_build(provider_id, || {
                let factory = Arc::clone(&self.factory);
                async move { factory.build(provider_id).await }
            })
            .await
            .map_err(|e| Status::internal(format!("failed to build SAML provider: {e}")))?;

        provider
            .validate_response(saml_response_b64)
            .map_err(|e| Status::unauthenticated(format!("SAML response invalid: {e}")))
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
        auth::{
            AuthProvider, OrgMembership, OrgRole, ProviderType, RefreshToken, User, UserStatus,
        },
        id::{AuthProviderId, OrganisationId, RefreshTokenId, UserId},
    };
    use stitchd_db::{
        AuthProviderRepository, AuthUserRepository, OrgMembershipRepository,
        RefreshTokenRepository, RepositoryError,
    };

    // ── MockSamlExchanger ────────────────────────────────────────────────────

    struct MockSamlExchanger {
        redirect_url: String,
        email: String,
        fail_validate: bool,
    }

    #[async_trait]
    impl SamlExchanger for MockSamlExchanger {
        async fn initiate_sso(
            &self,
            _provider_id: AuthProviderId,
            _acs_url: &str,
            _relay_state: &str,
        ) -> Result<String, Status> {
            Ok(self.redirect_url.clone())
        }

        async fn validate_response(
            &self,
            _provider_id: AuthProviderId,
            _saml_response_b64: &str,
        ) -> Result<String, Status> {
            if self.fail_validate {
                Err(Status::unauthenticated(
                    "SAML response invalid: bad signature",
                ))
            } else {
                Ok(self.email.clone())
            }
        }
    }

    // ── MockAuthProviderRepo ─────────────────────────────────────────────────

    struct MockAuthProviderRepo {
        provider: Option<AuthProvider>,
        org_providers: Vec<AuthProvider>,
    }

    fn make_saml_provider(org_id: OrganisationId) -> AuthProvider {
        AuthProvider {
            id: AuthProviderId::new(),
            org_id,
            provider_type: ProviderType::Saml,
            display_name: "Test SAML".to_string(),
            config: serde_json::json!({
                "idp_metadata_url": null,
                "idp_metadata_xml": "<EntityDescriptor/>",
                "name_id_format": "emailAddress",
                "sp_entity_id": "https://sp.example.com"
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

        async fn rotate_token_secret(&self, id: UserId) -> Result<uuid::Uuid, RepositoryError> {
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

        async fn list_org_users_paginated(
            &self,
            _org_id: OrganisationId,
            _offset: u64,
            _limit: u64,
        ) -> Result<(Vec<(User, OrgRole)>, u64), RepositoryError> {
            Ok((vec![], 0))
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
            Ok(OrgMembership {
                user_id,
                org_id,
                role,
                joined_at: Utc::now(),
            })
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
            _user_id: UserId,
        ) -> Result<Vec<OrgMembership>, RepositoryError> {
            Ok(self.membership.clone().map_or_else(Vec::new, |m| vec![m]))
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

        async fn find_by_hash(&self, _hash: &str) -> Result<Option<RefreshToken>, RepositoryError> {
            Ok(None)
        }

        async fn consume(
            &self,
            _id: RefreshTokenId,
        ) -> Result<Option<RefreshToken>, RepositoryError> {
            Ok(None)
        }

        async fn revoke(&self, _id: RefreshTokenId) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn revoke_all_for_user(&self, _user_id: UserId) -> Result<(), RepositoryError> {
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
        exchanger: impl SamlExchanger + 'static,
        provider: Option<AuthProvider>,
        org_providers: Vec<AuthProvider>,
        existing_user: Option<User>,
        membership: Option<OrgMembership>,
    ) -> SamlLoginServiceImpl {
        let org_id = provider
            .as_ref()
            .map_or_else(OrganisationId::new, |p| p.org_id);
        let membership = membership.or_else(|| {
            existing_user.as_ref().map(|u| OrgMembership {
                user_id: u.id,
                org_id,
                role: OrgRole::OrgMember,
                joined_at: Utc::now(),
            })
        });
        SamlLoginServiceImpl::new(
            Arc::new(exchanger),
            Arc::new(SamlRelayStore::new(Duration::from_secs(600))),
            Arc::new(MockAuthUserRepo { existing_user }),
            Arc::new(MockOrgMembershipRepo { membership }),
            Arc::new(MockRefreshTokenRepo),
            Arc::new(MockAuthProviderRepo {
                provider,
                org_providers,
            }),
        )
    }

    fn mock_exchanger() -> MockSamlExchanger {
        MockSamlExchanger {
            redirect_url: "https://idp.example.com/sso?SAMLRequest=xxx".to_string(),
            email: "user@example.com".to_string(),
            fail_validate: false,
        }
    }

    // ── Tests ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn sso_initiate_provider_scoped_returns_redirect_and_stores_relay_state() {
        let org_id = OrganisationId::new();
        let provider = make_saml_provider(org_id);
        let provider_id = provider.id;
        let svc = make_service(mock_exchanger(), Some(provider), vec![], None, None);

        let req = tonic::Request::new(SamlSsoRequest {
            scope: Some(Scope::ProviderId(provider_id.to_string())),
            acs_url: "https://sp.example.com/acs".to_string(),
        });

        let resp = svc.saml_sso_initiate(req).await;
        assert!(resp.is_ok(), "expected Ok, got: {resp:?}");
        let body = resp.unwrap().into_inner();
        assert!(
            body.redirect_url.contains("idp.example.com"),
            "redirect_url should be IdP URL: {}",
            body.redirect_url
        );
        assert!(
            !body.relay_state.is_empty(),
            "relay_state must be non-empty"
        );
        assert!(
            svc.relay_store.store.contains_key(&body.relay_state),
            "relay_state must be stored"
        );
    }

    #[tokio::test]
    async fn sso_initiate_org_scoped_picks_first_enabled_saml_provider() {
        let org_id = OrganisationId::new();
        let provider = make_saml_provider(org_id);
        let org_providers = vec![provider.clone()];
        let svc = make_service(mock_exchanger(), Some(provider), org_providers, None, None);

        let req = tonic::Request::new(SamlSsoRequest {
            scope: Some(Scope::OrgId(org_id.to_string())),
            acs_url: String::new(),
        });

        let resp = svc.saml_sso_initiate(req).await;
        assert!(resp.is_ok(), "org-scoped SAML SSO should succeed");
        assert!(!resp.unwrap().into_inner().redirect_url.is_empty());
    }

    #[tokio::test]
    async fn sso_initiate_org_scoped_no_saml_provider_returns_not_found() {
        let org_id = OrganisationId::new();
        let svc = make_service(mock_exchanger(), None, vec![], None, None);

        let req = tonic::Request::new(SamlSsoRequest {
            scope: Some(Scope::OrgId(org_id.to_string())),
            acs_url: String::new(),
        });

        let resp = svc.saml_sso_initiate(req).await;
        assert!(
            matches!(resp, Err(ref s) if s.code() == tonic::Code::NotFound),
            "expected NotFound, got: {resp:?}"
        );
    }

    #[tokio::test]
    async fn acs_callback_valid_assertion_issues_jwt() {
        let org_id = OrganisationId::new();
        let provider = make_saml_provider(org_id);
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

        // Plant relay state manually.
        let relay = "test-relay-state".to_string();
        svc.relay_store.insert(relay.clone(), provider.id);

        let req = tonic::Request::new(SamlAcsRequest {
            provider_id: provider.id.to_string(),
            saml_response_b64: "base64-saml-response".to_string(),
            relay_state: relay,
        });

        let resp = svc.saml_acs_callback(req).await;
        assert!(resp.is_ok(), "expected Ok, got: {resp:?}");
        let body = resp.unwrap().into_inner();
        assert!(
            !body.access_token.is_empty(),
            "access_token must be non-empty"
        );
        assert!(
            !body.refresh_token.is_empty(),
            "refresh_token must be non-empty"
        );
        assert_eq!(body.expires_in, 3600);
        assert_eq!(body.org_id, org_id.to_string());
    }

    #[tokio::test]
    async fn acs_callback_invalid_saml_response_returns_unauthenticated() {
        let org_id = OrganisationId::new();
        let provider = make_saml_provider(org_id);
        let svc = make_service(
            MockSamlExchanger {
                redirect_url: String::new(),
                email: String::new(),
                fail_validate: true,
            },
            Some(provider.clone()),
            vec![],
            None,
            None,
        );

        let relay = "relay-invalid-sig".to_string();
        svc.relay_store.insert(relay.clone(), provider.id);

        let req = tonic::Request::new(SamlAcsRequest {
            provider_id: provider.id.to_string(),
            saml_response_b64: "tampered-assertion".to_string(),
            relay_state: relay,
        });

        let resp = svc.saml_acs_callback(req).await;
        assert!(
            matches!(resp, Err(ref s) if s.code() == tonic::Code::Unauthenticated),
            "expected Unauthenticated for invalid SAML, got: {resp:?}"
        );
    }

    #[tokio::test]
    async fn acs_callback_replayed_relay_state_returns_unauthenticated() {
        let org_id = OrganisationId::new();
        let provider = make_saml_provider(org_id);
        let svc = make_service(mock_exchanger(), Some(provider), vec![], None, None);

        let req = tonic::Request::new(SamlAcsRequest {
            provider_id: String::new(),
            saml_response_b64: "anything".to_string(),
            relay_state: "relay-that-was-never-issued".to_string(),
        });

        let resp = svc.saml_acs_callback(req).await;
        assert!(
            matches!(resp, Err(ref s) if s.code() == tonic::Code::Unauthenticated),
            "expected Unauthenticated for replayed relay, got: {resp:?}"
        );
    }

    #[tokio::test]
    async fn acs_callback_expired_relay_state_returns_unauthenticated() {
        let org_id = OrganisationId::new();
        let provider = make_saml_provider(org_id);
        let svc = make_service(mock_exchanger(), Some(provider.clone()), vec![], None, None);

        // Plant expired relay state.
        svc.relay_store.store.insert(
            "expired-relay".to_string(),
            SamlPendingState {
                provider_id: provider.id,
                expiry: Instant::now()
                    .checked_sub(Duration::from_secs(1))
                    .expect("1s ago is valid"),
            },
        );

        let req = tonic::Request::new(SamlAcsRequest {
            provider_id: provider.id.to_string(),
            saml_response_b64: "valid-b64".to_string(),
            relay_state: "expired-relay".to_string(),
        });

        let resp = svc.saml_acs_callback(req).await;
        assert!(
            matches!(resp, Err(ref s) if s.code() == tonic::Code::Unauthenticated),
            "expected Unauthenticated for expired relay, got: {resp:?}"
        );
    }
}
