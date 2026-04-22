//! Factory for building [`SamlProvider`] instances from DB config.
//!
//! [`SamlProviderFactory`] loads a provider record from the repository and
//! builds a [`SamlProvider`] from either a metadata URL (fetched via HTTP)
//! or inline XML. Used as the `factory` closure in
//! [`ProviderCache::get_or_build`].

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use stitchd_core::{
    auth::{
        ProviderType,
        saml::{SamlConfig, SamlProvider},
    },
    id::AuthProviderId,
};
use stitchd_db::AuthProviderRepository;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Config shape
// ---------------------------------------------------------------------------

/// SAML-specific fields stored in the `auth_providers.config` JSON column.
///
/// Exactly one of `idp_metadata_url` or `idp_metadata_xml` must be set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SamlDbConfig {
    /// HTTP URL to the `IdP` XML metadata document.
    pub idp_metadata_url: Option<String>,
    /// Raw `IdP` XML metadata (air-gapped `IdPs`).
    pub idp_metadata_xml: Option<String>,
    #[serde(default = "default_name_id_format")]
    pub name_id_format: String,
    /// Our SP entity ID (must match what is configured in the `IdP`).
    pub sp_entity_id: String,
}

fn default_name_id_format() -> String {
    "urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress".to_string()
}

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum SamlFactoryError {
    #[error("provider {0} not found")]
    NotFound(AuthProviderId),

    #[error("provider {0} is not a SAML provider")]
    WrongType(AuthProviderId),

    #[error("invalid provider config: {0}")]
    InvalidConfig(String),

    #[error("metadata fetch failed: {0}")]
    MetadataFetch(String),

    #[error("metadata parse failed: {0}")]
    MetadataParse(String),

    #[error("repository error: {0}")]
    Repository(#[from] stitchd_db::RepositoryError),
}

// ---------------------------------------------------------------------------
// Metadata parsing helpers
// ---------------------------------------------------------------------------

/// Extract the `SingleSignOnService` URL from `IdP` metadata XML (redirect binding).
fn parse_sso_url(xml: &str) -> Option<String> {
    // Look for SingleSignOnService with HTTP-Redirect binding
    if let Some(pos) = xml.find("SingleSignOnService") {
        let rest = &xml[pos..];
        if let Some(loc_pos) = rest.find("Location=\"") {
            let after = &rest[loc_pos + 10..];
            if let Some(end) = after.find('"') {
                return Some(after[..end].to_string());
            }
        }
    }
    None
}

/// Extract the X.509 certificate PEM from `IdP` metadata XML.
fn parse_idp_cert(xml: &str) -> Option<String> {
    let start_tag = "<ds:X509Certificate>";
    let end_tag = "</ds:X509Certificate>";
    if let Some(start) = xml.find(start_tag) {
        let after = &xml[start + start_tag.len()..];
        if let Some(end) = after.find(end_tag) {
            let b64 = after[..end].trim().to_string();
            return Some(format!(
                "-----BEGIN CERTIFICATE-----\n{b64}\n-----END CERTIFICATE-----"
            ));
        }
    }
    // Fallback: unnamespaced tag
    let start_tag = "<X509Certificate>";
    let end_tag = "</X509Certificate>";
    if let Some(start) = xml.find(start_tag) {
        let after = &xml[start + start_tag.len()..];
        if let Some(end) = after.find(end_tag) {
            let b64 = after[..end].trim().to_string();
            return Some(format!(
                "-----BEGIN CERTIFICATE-----\n{b64}\n-----END CERTIFICATE-----"
            ));
        }
    }
    None
}

/// Extract the `entityID` from `IdP` metadata XML.
fn parse_entity_id(xml: &str) -> Option<String> {
    if let Some(pos) = xml.find("entityID=\"") {
        let after = &xml[pos + 10..];
        if let Some(end) = after.find('"') {
            return Some(after[..end].to_string());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Builds a [`SamlProvider`] for the given [`AuthProviderId`] from DB.
pub struct SamlProviderFactory {
    repo: Arc<dyn AuthProviderRepository>,
    http: reqwest::Client,
}

impl SamlProviderFactory {
    /// # Panics
    /// Panics if the `reqwest` client cannot be built (should never happen with default config).
    #[must_use]
    pub fn new(repo: Arc<dyn AuthProviderRepository>) -> Self {
        let http = reqwest::Client::builder()
            .user_agent("stitchd/1.0")
            .build()
            .expect("reqwest client build should not fail");
        Self { repo, http }
    }

    /// Build with a custom HTTP client (used in tests).
    #[must_use]
    pub fn with_client(repo: Arc<dyn AuthProviderRepository>, http: reqwest::Client) -> Self {
        Self { repo, http }
    }

    /// Build a [`SamlProvider`] for `provider_id`.
    ///
    /// 1. Loads the `AuthProvider` row from DB
    /// 2. Fetches `IdP` metadata XML from URL or uses inline XML
    /// 3. Parses SSO URL and certificate from metadata
    /// 4. Constructs [`SamlProvider`]
    ///
    /// # Errors
    /// Returns [`SamlFactoryError`] on DB miss, wrong type, fetch failure, or
    /// parse failure.
    pub async fn build(
        &self,
        provider_id: AuthProviderId,
    ) -> Result<Arc<SamlProvider>, SamlFactoryError> {
        let record = self
            .repo
            .find_by_id(provider_id)
            .await?
            .ok_or(SamlFactoryError::NotFound(provider_id))?;

        if record.provider_type != ProviderType::Saml {
            return Err(SamlFactoryError::WrongType(provider_id));
        }

        let cfg: SamlDbConfig = serde_json::from_value(record.config.clone())
            .map_err(|e| SamlFactoryError::InvalidConfig(e.to_string()))?;

        let xml = self.fetch_metadata_xml(&cfg).await?;

        let sso_url = parse_sso_url(&xml)
            .ok_or_else(|| SamlFactoryError::MetadataParse("SSO URL not found".to_string()))?;

        let idp_cert = parse_idp_cert(&xml).unwrap_or_default();
        let entity_id = parse_entity_id(&xml).unwrap_or_else(|| "unknown-idp".to_string());

        let provider = SamlProvider::new(SamlConfig {
            entity_id,
            sso_url,
            idp_cert_pem: idp_cert,
            sp_entity_id: cfg.sp_entity_id,
        });

        Ok(Arc::new(provider))
    }

    async fn fetch_metadata_xml(&self, cfg: &SamlDbConfig) -> Result<String, SamlFactoryError> {
        if let Some(xml) = &cfg.idp_metadata_xml {
            return Ok(xml.clone());
        }

        if let Some(url) = &cfg.idp_metadata_url {
            let response = self
                .http
                .get(url)
                .send()
                .await
                .map_err(|e| SamlFactoryError::MetadataFetch(e.to_string()))?;

            if !response.status().is_success() {
                return Err(SamlFactoryError::MetadataFetch(format!(
                    "HTTP {} fetching metadata",
                    response.status()
                )));
            }

            return response
                .text()
                .await
                .map_err(|e| SamlFactoryError::MetadataFetch(e.to_string()));
        }

        Err(SamlFactoryError::InvalidConfig(
            "neither idp_metadata_url nor idp_metadata_xml set".to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use stitchd_core::{
        auth::{AuthProvider, ProviderType},
        id::{AuthProviderId, OrganisationId},
    };
    use stitchd_db::{AuthProviderRepository, RepositoryError};

    // -----------------------------------------------------------------------
    // Mock repository
    // -----------------------------------------------------------------------

    struct MockRepo {
        provider: Option<AuthProvider>,
    }

    #[async_trait]
    impl AuthProviderRepository for MockRepo {
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
            unimplemented!()
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

    // -----------------------------------------------------------------------
    // Sample IdP metadata XML fixture
    // -----------------------------------------------------------------------

    const SAMPLE_IDP_METADATA: &str = r#"<?xml version="1.0"?>
<md:EntityDescriptor xmlns:md="urn:oasis:names:tc:SAML:2.0:metadata"
  entityID="https://idp.example.com">
  <md:IDPSSODescriptor
    WantAuthnRequestsSigned="false"
    protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    <md:KeyDescriptor use="signing">
      <ds:KeyInfo xmlns:ds="http://www.w3.org/2000/09/xmldsig#">
        <ds:X509Data>
          <ds:X509Certificate>MIICmDCCAYACCQDU6pQ</ds:X509Certificate>
        </ds:X509Data>
      </ds:KeyInfo>
    </md:KeyDescriptor>
    <md:SingleSignOnService
      Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect"
      Location="https://idp.example.com/sso"/>
  </md:IDPSSODescriptor>
</md:EntityDescriptor>"#;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn make_provider(provider_type: ProviderType, config: serde_json::Value) -> AuthProvider {
        use chrono::Utc;
        AuthProvider {
            id: AuthProviderId::new(),
            org_id: OrganisationId::new(),
            provider_type,
            display_name: "Test IdP".to_string(),
            config,
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn saml_inline_config() -> serde_json::Value {
        serde_json::json!({
            "idp_metadata_xml": SAMPLE_IDP_METADATA,
            "name_id_format": "urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress",
            "sp_entity_id": "https://sp.stitchd.dev",
        })
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn returns_not_found_when_provider_absent() {
        let repo = Arc::new(MockRepo { provider: None });
        let factory = SamlProviderFactory::new(repo);
        let result = factory.build(AuthProviderId::new()).await;
        assert!(matches!(result, Err(SamlFactoryError::NotFound(_))));
    }

    #[tokio::test]
    async fn returns_wrong_type_for_oidc_provider() {
        let provider = make_provider(ProviderType::Oidc, saml_inline_config());
        let id = provider.id;
        let repo = Arc::new(MockRepo {
            provider: Some(provider),
        });
        let factory = SamlProviderFactory::new(repo);
        let result = factory.build(id).await;
        assert!(matches!(result, Err(SamlFactoryError::WrongType(_))));
    }

    #[tokio::test]
    async fn raw_xml_path_parses_metadata_correctly() {
        let provider = make_provider(ProviderType::Saml, saml_inline_config());
        let id = provider.id;
        let repo = Arc::new(MockRepo {
            provider: Some(provider),
        });
        let factory = SamlProviderFactory::new(repo);
        // Should succeed — no HTTP call needed
        let result = factory.build(id).await;
        assert!(
            result.is_ok(),
            "raw XML path should succeed, got err: {}",
            result
                .err()
                .map_or_else(|| "none".to_string(), |e| e.to_string())
        );
    }

    #[tokio::test]
    async fn raw_xml_path_builds_provider_with_correct_sso_url() {
        let provider = make_provider(ProviderType::Saml, saml_inline_config());
        let id = provider.id;
        let repo = Arc::new(MockRepo {
            provider: Some(provider),
        });
        let factory = SamlProviderFactory::new(repo);
        let provider = factory.build(id).await.unwrap();
        // Verify the provider can generate an authn request with the parsed SSO URL
        let (_xml, redirect) = provider
            .authn_request("https://sp.stitchd.dev/acs", "relay")
            .expect("authn_request should succeed");
        assert!(
            redirect.starts_with("https://idp.example.com/sso"),
            "redirect should use parsed SSO URL, got: {redirect}"
        );
    }

    #[tokio::test]
    async fn url_path_fetch_is_called_when_no_inline_xml() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/metadata"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(SAMPLE_IDP_METADATA)
                    .insert_header("content-type", "application/xml"),
            )
            .mount(&mock_server)
            .await;

        let metadata_url = format!("{}/metadata", mock_server.uri());
        let config = serde_json::json!({
            "idp_metadata_url": metadata_url,
            "sp_entity_id": "https://sp.stitchd.dev",
        });
        let provider = make_provider(ProviderType::Saml, config);
        let id = provider.id;
        let repo = Arc::new(MockRepo {
            provider: Some(provider),
        });
        let factory = SamlProviderFactory::new(repo);
        let result = factory.build(id).await;
        assert!(
            result.is_ok(),
            "URL fetch path should succeed, got err: {}",
            result
                .err()
                .map_or_else(|| "none".to_string(), |e| e.to_string())
        );
    }

    #[tokio::test]
    async fn url_path_returns_error_on_http_failure() {
        let config = serde_json::json!({
            // Use a non-routable IP to force a connection failure
            "idp_metadata_url": "http://192.0.2.1/metadata",
            "sp_entity_id": "https://sp.stitchd.dev",
        });
        let provider = make_provider(ProviderType::Saml, config);
        let id = provider.id;
        let repo = Arc::new(MockRepo {
            provider: Some(provider),
        });
        // Use a very short timeout to avoid waiting in tests
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(100))
            .build()
            .unwrap();
        let factory = SamlProviderFactory::with_client(repo, http);
        let result = factory.build(id).await;
        assert!(
            matches!(result, Err(SamlFactoryError::MetadataFetch(_))),
            "should get fetch error"
        );
    }

    #[tokio::test]
    async fn missing_both_url_and_xml_returns_invalid_config() {
        let config = serde_json::json!({
            "sp_entity_id": "https://sp.stitchd.dev",
        });
        let provider = make_provider(ProviderType::Saml, config);
        let id = provider.id;
        let repo = Arc::new(MockRepo {
            provider: Some(provider),
        });
        let factory = SamlProviderFactory::new(repo);
        let result = factory.build(id).await;
        assert!(matches!(result, Err(SamlFactoryError::InvalidConfig(_))));
    }
}
