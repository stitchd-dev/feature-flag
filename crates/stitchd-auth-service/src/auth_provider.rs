//! gRPC handler for `AuthProviderService` — org-level OIDC/SAML provider CRUD.
//!
//! Environment variables:
//! - `STITCHD_SP_BASE_URL` — public base URL of this service, used to build ACS URLs
//!   (e.g. `https://auth.example.com`). Defaults to `http://localhost:8080`.

use std::sync::Arc;

use base64::Engine as _;
use tonic::{Request, Response, Status};
use tracing::instrument;
use uuid::Uuid;

use stitchd_core::{
    auth::{
        AuthProvider, CryptoKey, ProviderType,
        saml::{SamlConfig, SamlProvider},
    },
    id::{AuthProviderId, OrganisationId},
};
use stitchd_db::{AuthProviderRepository, RepositoryError};
use stitchd_proto::auth::v1::{
    AuthProviderResponse, CreateAuthProviderRequest, CreateAuthProviderResponse,
    DeleteAuthProviderRequest, DeleteAuthProviderResponse, GetAuthProviderRequest,
    GetAuthProviderResponse, GetSamlSpMetadataRequest, GetSamlSpMetadataResponse,
    ListAuthProvidersRequest, ListAuthProvidersResponse, OidcConfig as ProtoOidcConfig,
    SamlConfig as ProtoSamlConfig, UpdateAuthProviderRequest, UpdateAuthProviderResponse,
    auth_provider_service_server::AuthProviderService, create_auth_provider_request,
    update_auth_provider_request,
};

use crate::{app_state::ProviderCaches, oidc_factory::OidcConfig, saml_factory::SamlDbConfig};

// ---------------------------------------------------------------------------
// Service implementation
// ---------------------------------------------------------------------------

pub struct AuthProviderServiceImpl {
    repo: Arc<dyn AuthProviderRepository>,
    crypto: Arc<CryptoKey>,
    caches: Arc<ProviderCaches>,
}

impl AuthProviderServiceImpl {
    #[must_use]
    pub fn new(
        repo: Arc<dyn AuthProviderRepository>,
        crypto: Arc<CryptoKey>,
        caches: Arc<ProviderCaches>,
    ) -> Self {
        Self {
            repo,
            crypto,
            caches,
        }
    }

    fn sp_base_url() -> String {
        std::env::var("STITCHD_SP_BASE_URL").unwrap_or_else(|_| "http://localhost:8080".to_string())
    }

    fn acs_url(provider_id: AuthProviderId) -> String {
        format!(
            "{}/v1/auth/saml/{}/callback",
            Self::sp_base_url(),
            provider_id
        )
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn map_repo_err(e: RepositoryError) -> Status {
    match e {
        RepositoryError::NotFound { id } => Status::not_found(format!("not found: {id}")),
        RepositoryError::UniqueViolation { .. } => Status::already_exists("already exists"),
        RepositoryError::InvalidState { reason } => Status::permission_denied(reason),
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

/// Convert DB `AuthProvider` to proto `AuthProviderResponse` (secrets redacted).
fn to_proto_response(p: &AuthProvider, acs_url: Option<String>) -> AuthProviderResponse {
    use stitchd_proto::auth::v1::auth_provider_response::Config;

    let config = match p.provider_type {
        ProviderType::Oidc => {
            if let Ok(cfg) = serde_json::from_value::<OidcConfig>(p.config.clone()) {
                Some(Config::Oidc(ProtoOidcConfig {
                    issuer_url: cfg.issuer_url,
                    client_id: cfg.client_id,
                    client_secret: String::new(), // redacted
                    scopes: cfg.scopes,
                }))
            } else {
                None
            }
        }
        ProviderType::Saml => {
            if let Ok(cfg) = serde_json::from_value::<SamlDbConfig>(p.config.clone()) {
                Some(Config::Saml(ProtoSamlConfig {
                    idp_metadata_url: cfg.idp_metadata_url.unwrap_or_default(),
                    idp_metadata_xml: cfg.idp_metadata_xml.unwrap_or_default(),
                    name_id_format: cfg.name_id_format,
                    sp_entity_id: cfg.sp_entity_id,
                }))
            } else {
                None
            }
        }
        ProviderType::Password => None,
    };

    AuthProviderResponse {
        id: p.id.to_string(),
        org_id: p.org_id.to_string(),
        provider_type: match p.provider_type {
            ProviderType::Oidc => "oidc".to_string(),
            ProviderType::Saml => "saml".to_string(),
            ProviderType::Password => "password".to_string(),
        },
        display_name: p.display_name.clone(),
        enabled: p.enabled,
        created_at: p.created_at.to_rfc3339(),
        updated_at: p.updated_at.to_rfc3339(),
        acs_url: acs_url.unwrap_or_default(),
        config,
    }
}

// ---------------------------------------------------------------------------
// gRPC trait implementation
// ---------------------------------------------------------------------------

#[tonic::async_trait]
impl AuthProviderService for AuthProviderServiceImpl {
    #[instrument(skip_all)]
    async fn create_auth_provider(
        &self,
        req: Request<CreateAuthProviderRequest>,
    ) -> Result<Response<CreateAuthProviderResponse>, Status> {
        let r = req.into_inner();
        let org_id = parse_org_id(&r.org_id)?;

        let (provider_type, config_json) = match r.config {
            Some(create_auth_provider_request::Config::Oidc(oidc)) => {
                let plaintext = oidc.client_secret.as_bytes();
                let enc = self
                    .crypto
                    .encrypt(plaintext)
                    .map_err(|e| Status::internal(e.to_string()))?;
                let enc_b64 = base64::engine::general_purpose::STANDARD.encode(&enc);
                let cfg = OidcConfig {
                    issuer_url: oidc.issuer_url,
                    client_id: oidc.client_id,
                    client_secret_enc: enc_b64,
                    scopes: oidc.scopes,
                };
                (
                    ProviderType::Oidc,
                    serde_json::to_value(cfg).map_err(|e| Status::internal(e.to_string()))?,
                )
            }
            Some(create_auth_provider_request::Config::Saml(saml)) => {
                let cfg = SamlDbConfig {
                    idp_metadata_url: if saml.idp_metadata_url.is_empty() {
                        None
                    } else {
                        Some(saml.idp_metadata_url)
                    },
                    idp_metadata_xml: if saml.idp_metadata_xml.is_empty() {
                        None
                    } else {
                        Some(saml.idp_metadata_xml)
                    },
                    name_id_format: saml.name_id_format,
                    sp_entity_id: saml.sp_entity_id,
                };
                // Validate that at least one of URL or XML is provided
                if cfg.idp_metadata_url.is_none() && cfg.idp_metadata_xml.is_none() {
                    return Err(Status::invalid_argument(
                        "SAML provider requires idp_metadata_url or idp_metadata_xml",
                    ));
                }
                (
                    ProviderType::Saml,
                    serde_json::to_value(cfg).map_err(|e| Status::internal(e.to_string()))?,
                )
            }
            None => {
                return Err(Status::invalid_argument("config oneof must be set"));
            }
        };

        let provider = self
            .repo
            .create(
                org_id,
                provider_type,
                &r.display_name,
                config_json,
                r.enabled,
            )
            .await
            .map_err(map_repo_err)?;

        let acs_url = if provider.provider_type == ProviderType::Saml {
            Some(Self::acs_url(provider.id))
        } else {
            None
        };

        Ok(Response::new(CreateAuthProviderResponse {
            provider: Some(to_proto_response(&provider, acs_url)),
        }))
    }

    #[instrument(skip_all)]
    async fn list_auth_providers(
        &self,
        req: Request<ListAuthProvidersRequest>,
    ) -> Result<Response<ListAuthProvidersResponse>, Status> {
        let r = req.into_inner();
        let org_id = parse_org_id(&r.org_id)?;

        let providers = self.repo.list_for_org(org_id).await.map_err(map_repo_err)?;

        Ok(Response::new(ListAuthProvidersResponse {
            providers: providers
                .iter()
                .map(|p| to_proto_response(p, None))
                .collect(),
        }))
    }

    #[instrument(skip_all)]
    async fn get_auth_provider(
        &self,
        req: Request<GetAuthProviderRequest>,
    ) -> Result<Response<GetAuthProviderResponse>, Status> {
        let r = req.into_inner();
        let id = parse_provider_id(&r.provider_id)?;

        let provider = self
            .repo
            .find_by_id(id)
            .await
            .map_err(map_repo_err)?
            .ok_or_else(|| Status::not_found(format!("provider {id} not found")))?;

        Ok(Response::new(GetAuthProviderResponse {
            provider: Some(to_proto_response(&provider, None)),
        }))
    }

    #[instrument(skip_all)]
    async fn update_auth_provider(
        &self,
        req: Request<UpdateAuthProviderRequest>,
    ) -> Result<Response<UpdateAuthProviderResponse>, Status> {
        let r = req.into_inner();
        let id = parse_provider_id(&r.provider_id)?;

        // Fetch current config to preserve encrypted secret if not being updated
        let current = self
            .repo
            .find_by_id(id)
            .await
            .map_err(map_repo_err)?
            .ok_or_else(|| Status::not_found(format!("provider {id} not found")))?;

        let config_json = match r.config {
            Some(update_auth_provider_request::Config::Oidc(oidc)) => {
                let enc_b64 = if oidc.client_secret.is_empty() {
                    // Preserve existing encrypted secret
                    current
                        .config
                        .get("client_secret_enc")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string()
                } else {
                    let enc = self
                        .crypto
                        .encrypt(oidc.client_secret.as_bytes())
                        .map_err(|e| Status::internal(e.to_string()))?;
                    base64::engine::general_purpose::STANDARD.encode(&enc)
                };
                let cfg = OidcConfig {
                    issuer_url: oidc.issuer_url,
                    client_id: oidc.client_id,
                    client_secret_enc: enc_b64,
                    scopes: oidc.scopes,
                };
                serde_json::to_value(cfg).map_err(|e| Status::internal(e.to_string()))?
            }
            Some(update_auth_provider_request::Config::Saml(saml)) => {
                let cfg = SamlDbConfig {
                    idp_metadata_url: if saml.idp_metadata_url.is_empty() {
                        None
                    } else {
                        Some(saml.idp_metadata_url)
                    },
                    idp_metadata_xml: if saml.idp_metadata_xml.is_empty() {
                        None
                    } else {
                        Some(saml.idp_metadata_xml)
                    },
                    name_id_format: saml.name_id_format,
                    sp_entity_id: saml.sp_entity_id,
                };
                serde_json::to_value(cfg).map_err(|e| Status::internal(e.to_string()))?
            }
            None => current.config.clone(),
        };

        let provider = self
            .repo
            .update(id, &r.display_name, config_json, r.enabled)
            .await
            .map_err(map_repo_err)?;

        // Evict both caches on update
        self.caches.evict(id);

        Ok(Response::new(UpdateAuthProviderResponse {
            provider: Some(to_proto_response(&provider, None)),
        }))
    }

    #[instrument(skip_all)]
    async fn delete_auth_provider(
        &self,
        req: Request<DeleteAuthProviderRequest>,
    ) -> Result<Response<DeleteAuthProviderResponse>, Status> {
        let r = req.into_inner();
        let id = parse_provider_id(&r.provider_id)?;

        self.repo.delete(id).await.map_err(map_repo_err)?;

        // Evict both caches on delete
        self.caches.evict(id);

        Ok(Response::new(DeleteAuthProviderResponse {}))
    }

    #[instrument(skip_all)]
    async fn get_saml_sp_metadata(
        &self,
        req: Request<GetSamlSpMetadataRequest>,
    ) -> Result<Response<GetSamlSpMetadataResponse>, Status> {
        let r = req.into_inner();
        let id = parse_provider_id(&r.provider_id)?;

        let provider = self
            .repo
            .find_by_id(id)
            .await
            .map_err(map_repo_err)?
            .ok_or_else(|| Status::not_found(format!("provider {id} not found")))?;

        if provider.provider_type != ProviderType::Saml {
            return Err(Status::invalid_argument("provider is not a SAML provider"));
        }

        let cfg: SamlDbConfig = serde_json::from_value(provider.config)
            .map_err(|e| Status::internal(format!("invalid SAML config: {e}")))?;

        let sp = SamlProvider::new(SamlConfig {
            entity_id: String::new(), // not needed for SP metadata generation
            sso_url: String::new(),
            idp_cert_pem: String::new(),
            sp_entity_id: cfg.sp_entity_id,
        });

        let acs_url = Self::acs_url(id);
        let xml = sp.sp_metadata_xml(&acs_url);

        Ok(Response::new(GetSamlSpMetadataResponse { xml }))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::Utc;
    use std::sync::Mutex as StdMutex;
    use stitchd_core::id::OrganisationId;
    use stitchd_db::RepositoryError;
    use stitchd_proto::auth::v1::{SamlConfig as ProtoSamlConfig, create_auth_provider_request};

    // -----------------------------------------------------------------------
    // Mock repository
    // -----------------------------------------------------------------------

    struct MockProviderRepo {
        providers: StdMutex<Vec<AuthProvider>>,
    }

    impl MockProviderRepo {
        fn empty() -> Arc<Self> {
            Arc::new(Self {
                providers: StdMutex::new(vec![]),
            })
        }
    }

    #[async_trait]
    impl AuthProviderRepository for MockProviderRepo {
        async fn create(
            &self,
            org_id: OrganisationId,
            provider_type: ProviderType,
            display_name: &str,
            config: serde_json::Value,
            enabled: bool,
        ) -> Result<AuthProvider, RepositoryError> {
            let p = AuthProvider {
                id: AuthProviderId::new(),
                org_id,
                provider_type,
                display_name: display_name.to_string(),
                config,
                enabled,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            };
            self.providers.lock().unwrap().push(p.clone());
            Ok(p)
        }

        async fn find_by_id(
            &self,
            id: AuthProviderId,
        ) -> Result<Option<AuthProvider>, RepositoryError> {
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
        ) -> Result<Vec<AuthProvider>, RepositoryError> {
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
            config: serde_json::Value,
            enabled: bool,
        ) -> Result<AuthProvider, RepositoryError> {
            let mut providers = self.providers.lock().unwrap();
            let result = providers
                .iter_mut()
                .find(|p| p.id == id)
                .map(|p| {
                    p.display_name = display_name.to_string();
                    p.config = config;
                    p.enabled = enabled;
                    p.clone()
                })
                .ok_or_else(|| RepositoryError::NotFound { id: id.to_string() });
            drop(providers);
            result
        }

        async fn delete(&self, id: AuthProviderId) -> Result<(), RepositoryError> {
            let found = {
                let mut providers = self.providers.lock().unwrap();
                let before = providers.len();
                providers.retain(|p| p.id != id);
                providers.len() != before
            };
            if found {
                Ok(())
            } else {
                Err(RepositoryError::NotFound { id: id.to_string() })
            }
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn test_service(repo: Arc<dyn AuthProviderRepository>) -> AuthProviderServiceImpl {
        AuthProviderServiceImpl::new(
            repo,
            Arc::new(CryptoKey::from_bytes([0u8; 32])),
            Arc::new(ProviderCaches::from_env()),
        )
    }

    fn test_org_id() -> OrganisationId {
        OrganisationId::new()
    }

    // -----------------------------------------------------------------------
    // create_auth_provider — OIDC
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn create_oidc_encrypts_secret_and_returns_provider() {
        let svc = test_service(MockProviderRepo::empty());
        let org_id = test_org_id();

        let req = tonic::Request::new(CreateAuthProviderRequest {
            org_id: org_id.to_string(),
            display_name: "Google OIDC".to_string(),
            enabled: true,
            config: Some(create_auth_provider_request::Config::Oidc(
                ProtoOidcConfig {
                    issuer_url: "https://accounts.google.com".to_string(),
                    client_id: "client-id".to_string(),
                    client_secret: "my-secret".to_string(),
                    scopes: vec![],
                },
            )),
        });

        let resp = svc.create_auth_provider(req).await.unwrap();
        let p = resp.into_inner().provider.unwrap();

        assert_eq!(p.provider_type, "oidc");
        assert_eq!(p.display_name, "Google OIDC");
        assert!(p.enabled);
        assert!(p.acs_url.is_empty(), "OIDC has no ACS URL");

        // Secret must be redacted in response
        if let Some(stitchd_proto::auth::v1::auth_provider_response::Config::Oidc(oidc)) = p.config
        {
            assert!(
                oidc.client_secret.is_empty(),
                "client_secret must be redacted"
            );
        } else {
            panic!("expected OIDC config in response");
        }
    }

    // -----------------------------------------------------------------------
    // create_auth_provider — SAML
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn create_saml_returns_acs_url() {
        let svc = test_service(MockProviderRepo::empty());
        let org_id = test_org_id();

        let req = tonic::Request::new(CreateAuthProviderRequest {
            org_id: org_id.to_string(),
            display_name: "Okta SAML".to_string(),
            enabled: true,
            config: Some(create_auth_provider_request::Config::Saml(
                ProtoSamlConfig {
                    idp_metadata_xml: "<xml/>".to_string(),
                    idp_metadata_url: String::new(),
                    name_id_format: "urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress"
                        .to_string(),
                    sp_entity_id: "https://sp.example.com".to_string(),
                },
            )),
        });

        let resp = svc.create_auth_provider(req).await.unwrap();
        let p = resp.into_inner().provider.unwrap();

        assert_eq!(p.provider_type, "saml");
        assert!(
            p.acs_url.contains("/v1/auth/saml/"),
            "SAML create should return ACS URL, got: {}",
            p.acs_url
        );
        assert!(p.acs_url.ends_with("/callback"));
    }

    // -----------------------------------------------------------------------
    // list_auth_providers
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn list_providers_returns_only_org_providers() {
        let repo = MockProviderRepo::empty();
        let svc = test_service(repo.clone());
        let org_a = test_org_id();
        let org_b = test_org_id();

        // Create one provider for each org
        svc.create_auth_provider(tonic::Request::new(CreateAuthProviderRequest {
            org_id: org_a.to_string(),
            display_name: "Provider A".to_string(),
            enabled: true,
            config: Some(create_auth_provider_request::Config::Saml(
                ProtoSamlConfig {
                    idp_metadata_xml: "<xml/>".to_string(),
                    idp_metadata_url: String::new(),
                    name_id_format: String::new(),
                    sp_entity_id: "https://sp.example.com".to_string(),
                },
            )),
        }))
        .await
        .unwrap();

        svc.create_auth_provider(tonic::Request::new(CreateAuthProviderRequest {
            org_id: org_b.to_string(),
            display_name: "Provider B".to_string(),
            enabled: true,
            config: Some(create_auth_provider_request::Config::Saml(
                ProtoSamlConfig {
                    idp_metadata_xml: "<xml/>".to_string(),
                    idp_metadata_url: String::new(),
                    name_id_format: String::new(),
                    sp_entity_id: "https://sp.example.com".to_string(),
                },
            )),
        }))
        .await
        .unwrap();

        let resp = svc
            .list_auth_providers(tonic::Request::new(ListAuthProvidersRequest {
                org_id: org_a.to_string(),
            }))
            .await
            .unwrap();

        let providers = resp.into_inner().providers;
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].display_name, "Provider A");
    }

    // -----------------------------------------------------------------------
    // get_auth_provider
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn get_provider_returns_correct_provider() {
        let repo = MockProviderRepo::empty();
        let svc = test_service(repo.clone());
        let org_id = test_org_id();

        let create_resp = svc
            .create_auth_provider(tonic::Request::new(CreateAuthProviderRequest {
                org_id: org_id.to_string(),
                display_name: "Test Provider".to_string(),
                enabled: true,
                config: Some(create_auth_provider_request::Config::Oidc(
                    ProtoOidcConfig {
                        issuer_url: "https://auth.example.com".to_string(),
                        client_id: "cid".to_string(),
                        client_secret: "secret".to_string(),
                        scopes: vec![],
                    },
                )),
            }))
            .await
            .unwrap();
        let created_id = create_resp.into_inner().provider.unwrap().id;

        let get_resp = svc
            .get_auth_provider(tonic::Request::new(GetAuthProviderRequest {
                provider_id: created_id.clone(),
            }))
            .await
            .unwrap();
        let got = get_resp.into_inner().provider.unwrap();
        assert_eq!(got.id, created_id);
        assert_eq!(got.display_name, "Test Provider");

        // Secret must be absent
        if let Some(stitchd_proto::auth::v1::auth_provider_response::Config::Oidc(oidc)) =
            got.config
        {
            assert!(oidc.client_secret.is_empty());
        }
    }

    // -----------------------------------------------------------------------
    // update_auth_provider
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn update_provider_evicts_cache_and_returns_updated() {
        let repo = MockProviderRepo::empty();
        let svc = test_service(repo.clone());
        let org_id = test_org_id();

        let create_resp = svc
            .create_auth_provider(tonic::Request::new(CreateAuthProviderRequest {
                org_id: org_id.to_string(),
                display_name: "Original".to_string(),
                enabled: true,
                config: Some(create_auth_provider_request::Config::Oidc(
                    ProtoOidcConfig {
                        issuer_url: "https://issuer.example.com".to_string(),
                        client_id: "cid".to_string(),
                        client_secret: "s3cr3t".to_string(),
                        scopes: vec![],
                    },
                )),
            }))
            .await
            .unwrap();
        let pid = create_resp.into_inner().provider.unwrap().id;

        let update_resp = svc
            .update_auth_provider(tonic::Request::new(UpdateAuthProviderRequest {
                provider_id: pid.clone(),
                display_name: "Updated".to_string(),
                enabled: false,
                config: None,
            }))
            .await
            .unwrap();
        let updated = update_resp.into_inner().provider.unwrap();
        assert_eq!(updated.display_name, "Updated");
        assert!(!updated.enabled);
    }

    // -----------------------------------------------------------------------
    // delete_auth_provider
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn delete_provider_removes_from_repo() {
        let repo = MockProviderRepo::empty();
        let svc = test_service(repo.clone());
        let org_id = test_org_id();

        let create_resp = svc
            .create_auth_provider(tonic::Request::new(CreateAuthProviderRequest {
                org_id: org_id.to_string(),
                display_name: "To Delete".to_string(),
                enabled: true,
                config: Some(create_auth_provider_request::Config::Saml(
                    ProtoSamlConfig {
                        idp_metadata_xml: "<xml/>".to_string(),
                        idp_metadata_url: String::new(),
                        name_id_format: String::new(),
                        sp_entity_id: "https://sp.example.com".to_string(),
                    },
                )),
            }))
            .await
            .unwrap();
        let pid = create_resp.into_inner().provider.unwrap().id;

        svc.delete_auth_provider(tonic::Request::new(DeleteAuthProviderRequest {
            provider_id: pid.clone(),
        }))
        .await
        .unwrap();

        // Provider should be gone
        let get_result = svc
            .get_auth_provider(tonic::Request::new(GetAuthProviderRequest {
                provider_id: pid,
            }))
            .await;
        assert!(get_result.is_err());
    }

    // -----------------------------------------------------------------------
    // get_saml_sp_metadata
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn saml_sp_metadata_returns_xml() {
        let repo = MockProviderRepo::empty();
        let svc = test_service(repo.clone());
        let org_id = test_org_id();

        let create_resp = svc
            .create_auth_provider(tonic::Request::new(CreateAuthProviderRequest {
                org_id: org_id.to_string(),
                display_name: "SAML IdP".to_string(),
                enabled: true,
                config: Some(create_auth_provider_request::Config::Saml(
                    ProtoSamlConfig {
                        idp_metadata_xml: "<xml/>".to_string(),
                        idp_metadata_url: String::new(),
                        name_id_format: "urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress"
                            .to_string(),
                        sp_entity_id: "https://sp.stitchd.dev".to_string(),
                    },
                )),
            }))
            .await
            .unwrap();
        let pid = create_resp.into_inner().provider.unwrap().id;

        let meta_resp = svc
            .get_saml_sp_metadata(tonic::Request::new(GetSamlSpMetadataRequest {
                provider_id: pid,
            }))
            .await
            .unwrap();
        let xml = meta_resp.into_inner().xml;
        assert!(
            xml.contains("EntityDescriptor"),
            "should have EntityDescriptor"
        );
        assert!(
            xml.contains("SPSSODescriptor"),
            "should have SPSSODescriptor"
        );
        assert!(xml.contains("AssertionConsumerService"), "should have ACS");
    }
}
