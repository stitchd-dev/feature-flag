//! SAML integration tests — mocked IdP metadata (inline XML fixture).
//!
//! Tests the full SAML login flow using real `SamlProviderFactory`,
//! `LiveSamlExchanger`, and `SamlLoginServiceImpl` with mock repositories.
//!
//! No cryptographic signature verification is performed by the current
//! `SamlProvider` implementation (see `stitchd-core::auth::saml` module doc),
//! so any structurally valid SAMLResponse XML works in these tests.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use base64::Engine as _;
use chrono::Utc;
use tonic::Request;

use stitchd_auth_service::{
    app_state::ProviderCaches,
    provider_cache::ProviderCache,
    saml_factory::SamlProviderFactory,
    saml_login::{LiveSamlExchanger, SamlLoginServiceImpl, SamlRelayStore},
};
use stitchd_core::{
    auth::{
        AuthProvider, OidcProvider, OrgMembership, OrgRole, ProviderType, RefreshToken, User,
        UserStatus, saml::SamlProvider,
    },
    id::{AuthProviderId, OrganisationId, RefreshTokenId, UserId},
};
use stitchd_db::{
    AuthProviderRepository, AuthUserRepository, OrgMembershipRepository, RefreshTokenRepository,
    RepositoryError,
};
use stitchd_proto::auth::v1::saml_login_service_server::SamlLoginService as _;
use stitchd_proto::auth::v1::{SamlAcsRequest, SamlSsoRequest, saml_sso_request::Scope};

// ─── Test IdP metadata XML ───────────────────────────────────────────────────

const IDP_METADATA_XML: &str = r#"<?xml version="1.0"?>
<md:EntityDescriptor xmlns:md="urn:oasis:names:tc:SAML:2.0:metadata"
  entityID="https://idp.integration-test.example.com">
  <md:IDPSSODescriptor
    WantAuthnRequestsSigned="false"
    protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    <md:KeyDescriptor use="signing">
      <ds:KeyInfo xmlns:ds="http://www.w3.org/2000/09/xmldsig#">
        <ds:X509Data>
          <ds:X509Certificate>MIICmDCCAYACCQDU6pQtest==</ds:X509Certificate>
        </ds:X509Data>
      </ds:KeyInfo>
    </md:KeyDescriptor>
    <md:SingleSignOnService
      Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect"
      Location="https://idp.integration-test.example.com/sso"/>
  </md:IDPSSODescriptor>
</md:EntityDescriptor>"#;

/// Build a minimal but valid base64-encoded SAMLResponse for the given email.
fn make_saml_response_b64(email: &str) -> String {
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<samlp:Response xmlns:samlp="urn:oasis:names:tc:SAML:2.0:protocol"
  xmlns:saml="urn:oasis:names:tc:SAML:2.0:assertion"
  ID="_response001" Version="2.0"
  IssueInstant="2026-04-22T00:00:00Z">
  <saml:Issuer>https://idp.integration-test.example.com</saml:Issuer>
  <samlp:Status>
    <samlp:StatusCode Value="urn:oasis:names:tc:SAML:2.0:status:Success"/>
  </samlp:Status>
  <saml:Assertion ID="_assertion001" Version="2.0"
    IssueInstant="2026-04-22T00:00:00Z">
    <saml:Issuer>https://idp.integration-test.example.com</saml:Issuer>
    <saml:Subject>
      <saml:NameID Format="urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress">{email}</saml:NameID>
    </saml:Subject>
  </saml:Assertion>
</samlp:Response>"#
    );
    base64::engine::general_purpose::STANDARD.encode(xml.as_bytes())
}

// ─── Mock repositories ────────────────────────────────────────────────────────

struct MockAuthProviderRepo {
    provider: AuthProvider,
}

#[async_trait]
impl AuthProviderRepository for MockAuthProviderRepo {
    async fn create(
        &self,
        _org_id: OrganisationId,
        _provider_type: ProviderType,
        _display_name: &str,
        _config: serde_json::Value,
        _enabled: bool,
    ) -> Result<AuthProvider, RepositoryError> {
        unimplemented!()
    }

    async fn find_by_id(
        &self,
        _id: AuthProviderId,
    ) -> Result<Option<AuthProvider>, RepositoryError> {
        Ok(Some(self.provider.clone()))
    }

    async fn list_for_org(&self, _: OrganisationId) -> Result<Vec<AuthProvider>, RepositoryError> {
        Ok(vec![self.provider.clone()])
    }

    async fn update(
        &self,
        _id: AuthProviderId,
        _display_name: &str,
        _config: serde_json::Value,
        _enabled: bool,
    ) -> Result<AuthProvider, RepositoryError> {
        unimplemented!()
    }

    async fn delete(&self, _id: AuthProviderId) -> Result<(), RepositoryError> {
        unimplemented!()
    }
}

struct MockUserRepo;

fn make_test_user(email: &str) -> User {
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
impl AuthUserRepository for MockUserRepo {
    async fn create(
        &self,
        email: &str,
        _display_name: &str,
        _password_hash: Option<&str>,
    ) -> Result<User, RepositoryError> {
        Ok(make_test_user(email))
    }

    async fn find_by_email(&self, _email: &str) -> Result<Option<User>, RepositoryError> {
        Ok(None) // Always create a new user
    }

    async fn find_by_id(&self, _id: UserId) -> Result<Option<User>, RepositoryError> {
        Ok(None)
    }

    async fn rotate_token_secret(&self, id: UserId) -> Result<uuid::Uuid, RepositoryError> {
        Err(RepositoryError::NotFound { id: id.to_string() })
    }

    async fn update_status(&self, _id: UserId, _status: UserStatus) -> Result<(), RepositoryError> {
        Ok(())
    }

    async fn update_password_hash(&self, _id: UserId, _hash: &str) -> Result<(), RepositoryError> {
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

struct MockMembershipRepo {
    #[allow(dead_code)]
    org_id: OrganisationId,
}

#[async_trait]
impl OrgMembershipRepository for MockMembershipRepo {
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
        Ok(None) // Always create membership
    }

    async fn list_orgs_for_user(
        &self,
        _user_id: UserId,
    ) -> Result<Vec<OrgMembership>, RepositoryError> {
        Ok(vec![])
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
            expires_at: Utc::now() + chrono::Duration::days(30),
            revoked_at: None,
            last_used_at: None,
        };
        Ok((rt, "raw-refresh-token".to_string()))
    }

    async fn find_by_hash(&self, _hash: &str) -> Result<Option<RefreshToken>, RepositoryError> {
        Ok(None)
    }

    async fn consume(&self, _id: RefreshTokenId) -> Result<Option<RefreshToken>, RepositoryError> {
        Ok(None)
    }

    async fn revoke(&self, _id: RefreshTokenId) -> Result<(), RepositoryError> {
        Ok(())
    }

    async fn revoke_all_for_user(&self, _user_id: UserId) -> Result<(), RepositoryError> {
        Ok(())
    }

    async fn list_active(&self, _user_id: UserId) -> Result<Vec<RefreshToken>, RepositoryError> {
        Ok(vec![])
    }
}

// ─── Fixture builder ──────────────────────────────────────────────────────────

struct TestFixture {
    provider_id: AuthProviderId,
    org_id: OrganisationId,
    service: SamlLoginServiceImpl,
}

fn build_fixture() -> TestFixture {
    let org_id = OrganisationId::new();
    let provider_id = AuthProviderId::new();

    let provider = AuthProvider {
        id: provider_id,
        org_id,
        provider_type: ProviderType::Saml,
        display_name: "Integration Test SAML".to_string(),
        config: serde_json::json!({
            "idp_metadata_xml": IDP_METADATA_XML,
            "name_id_format": "urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress",
            "sp_entity_id": "https://sp.integration-test.example.com",
        }),
        enabled: true,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };

    let provider_repo: Arc<dyn AuthProviderRepository> =
        Arc::new(MockAuthProviderRepo { provider });

    let factory = Arc::new(SamlProviderFactory::new(Arc::clone(&provider_repo)));
    let caches = Arc::new(ProviderCaches {
        oidc: ProviderCache::<Arc<OidcProvider>>::new(Duration::from_secs(3600)),
        saml: ProviderCache::<Arc<SamlProvider>>::new(Duration::from_secs(3600)),
    });
    let exchanger = Arc::new(LiveSamlExchanger::new(caches, factory));
    let relay_store = Arc::new(SamlRelayStore::new(Duration::from_secs(600)));

    let service = SamlLoginServiceImpl::new(
        exchanger,
        relay_store,
        Arc::new(MockUserRepo),
        Arc::new(MockMembershipRepo { org_id }),
        Arc::new(MockRefreshTokenRepo),
        provider_repo,
    );

    TestFixture {
        provider_id,
        org_id,
        service,
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

/// SSO initiation returns a redirect URL pointing at the IdP's SSO endpoint.
#[tokio::test]
async fn saml_sso_initiate_returns_idp_redirect_url() {
    let fix = build_fixture();

    let req = Request::new(SamlSsoRequest {
        scope: Some(Scope::ProviderId(fix.provider_id.to_string())),
        acs_url: "https://sp.integration-test.example.com/acs".to_string(),
    });

    let resp = fix
        .service
        .saml_sso_initiate(req)
        .await
        .expect("SSO initiate should succeed");
    let body = resp.into_inner();

    assert!(
        body.redirect_url
            .contains("idp.integration-test.example.com/sso"),
        "redirect_url should point to the IdP SSO endpoint, got: {}",
        body.redirect_url
    );
    assert!(
        body.redirect_url.contains("SAMLRequest="),
        "redirect_url should include SAMLRequest param"
    );
    assert!(
        !body.relay_state.is_empty(),
        "relay_state must be non-empty"
    );
}

/// Org-scoped SSO initiation picks the first enabled SAML provider for the org.
#[tokio::test]
async fn saml_sso_initiate_org_scoped_picks_enabled_provider() {
    let fix = build_fixture();

    let req = Request::new(SamlSsoRequest {
        scope: Some(Scope::OrgId(fix.org_id.to_string())),
        acs_url: "".to_string(), // Use default ACS URL
    });

    let resp = fix
        .service
        .saml_sso_initiate(req)
        .await
        .expect("org-scoped SSO should succeed");
    let body = resp.into_inner();

    assert!(
        body.redirect_url
            .contains("idp.integration-test.example.com"),
        "redirect URL should use IdP from org's provider"
    );
}

/// A valid ACS callback extracts the email from the SAMLResponse and issues a JWT.
#[tokio::test]
async fn saml_acs_callback_issues_jwt_for_valid_response() {
    let fix = build_fixture();

    // Plant a relay state manually (simulates the state stored during SSO initiation)
    let relay = "integration-test-relay-state".to_string();
    fix.service
        .relay_store()
        .insert(relay.clone(), fix.provider_id);

    let saml_response = make_saml_response_b64("saml-user@integration-test.example.com");

    let req = Request::new(SamlAcsRequest {
        provider_id: fix.provider_id.to_string(),
        saml_response_b64: saml_response,
        relay_state: relay,
    });

    let resp = fix
        .service
        .saml_acs_callback(req)
        .await
        .expect("ACS callback should succeed");
    let body = resp.into_inner();

    assert!(
        !body.access_token.is_empty(),
        "access_token must be non-empty"
    );
    assert!(
        !body.refresh_token.is_empty(),
        "refresh_token must be non-empty"
    );
    assert_eq!(body.expires_in, 3600);
    assert!(!body.user_id.is_empty(), "user_id must be set");
    assert_eq!(body.org_id, fix.org_id.to_string());
}

/// Full end-to-end flow: SSO initiate → use relay state → ACS callback → JWT.
#[tokio::test]
async fn saml_full_flow_sso_to_jwt() {
    let fix = build_fixture();

    // Step 1: Initiate SSO
    let sso_req = Request::new(SamlSsoRequest {
        scope: Some(Scope::ProviderId(fix.provider_id.to_string())),
        acs_url: "https://sp.integration-test.example.com/acs".to_string(),
    });
    let sso_resp = fix
        .service
        .saml_sso_initiate(sso_req)
        .await
        .expect("SSO initiate should succeed");
    let relay_state = sso_resp.into_inner().relay_state;

    // Step 2: Simulate IdP posting back to ACS endpoint
    let saml_response = make_saml_response_b64("flow-user@integration-test.example.com");
    let acs_req = Request::new(SamlAcsRequest {
        provider_id: fix.provider_id.to_string(),
        saml_response_b64: saml_response,
        relay_state,
    });
    let acs_resp = fix
        .service
        .saml_acs_callback(acs_req)
        .await
        .expect("ACS callback should succeed");
    let tokens = acs_resp.into_inner();

    assert!(!tokens.access_token.is_empty(), "must get access_token");
    assert!(!tokens.refresh_token.is_empty(), "must get refresh_token");
    assert_eq!(tokens.org_id, fix.org_id.to_string());
}

/// Replaying the same relay state (after it was consumed) is rejected.
#[tokio::test]
async fn saml_full_flow_relay_state_cannot_be_replayed() {
    let fix = build_fixture();

    // SSO initiate to get a real relay state
    let sso_req = Request::new(SamlSsoRequest {
        scope: Some(Scope::ProviderId(fix.provider_id.to_string())),
        acs_url: "https://sp.integration-test.example.com/acs".to_string(),
    });
    let relay_state = fix
        .service
        .saml_sso_initiate(sso_req)
        .await
        .unwrap()
        .into_inner()
        .relay_state;

    let saml_response = make_saml_response_b64("replay-user@example.com");

    // First ACS callback — succeeds
    let acs1 = Request::new(SamlAcsRequest {
        provider_id: fix.provider_id.to_string(),
        saml_response_b64: saml_response.clone(),
        relay_state: relay_state.clone(),
    });
    fix.service
        .saml_acs_callback(acs1)
        .await
        .expect("first ACS callback should succeed");

    // Second ACS callback with same relay state — relay consumed, must fail
    let acs2 = Request::new(SamlAcsRequest {
        provider_id: fix.provider_id.to_string(),
        saml_response_b64: saml_response,
        relay_state,
    });
    let result = fix.service.saml_acs_callback(acs2).await;
    assert!(
        matches!(result, Err(ref s) if s.code() == tonic::Code::Unauthenticated),
        "replayed relay state must be rejected, got: {result:?}"
    );
}
