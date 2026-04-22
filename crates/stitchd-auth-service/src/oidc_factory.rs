//! Factory for building [`OidcProvider`] instances from DB config.
//!
//! [`OidcProviderFactory`] loads a provider record from the repository,
//! decrypts the `client_secret`, and calls [`OidcProvider::from_discovery`].
//! Used as the `factory` closure in [`ProviderCache::get_or_build`].

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use stitchd_core::{
    auth::{CryptoKey, OidcError, OidcProvider, ProviderType},
    id::AuthProviderId,
};
use stitchd_db::AuthProviderRepository;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Config shape
// ---------------------------------------------------------------------------

/// OIDC-specific fields stored in the `auth_providers.config` JSON column.
///
/// `client_secret_enc` is the AES-256-GCM encrypted secret, stored as
/// base64-encoded bytes (nonce || ciphertext).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcConfig {
    pub issuer_url: String,
    pub client_id: String,
    /// AES-256-GCM encrypted secret (base64: nonce || ciphertext).
    pub client_secret_enc: String,
    #[serde(default)]
    pub scopes: Vec<String>,
}

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum FactoryError {
    #[error("provider {0} not found")]
    NotFound(AuthProviderId),

    #[error("provider {0} is not an OIDC provider")]
    WrongType(AuthProviderId),

    #[error("invalid provider config: {0}")]
    InvalidConfig(String),

    #[error("decryption failed: {0}")]
    Decryption(String),

    #[error("OIDC discovery failed: {0}")]
    Discovery(#[from] OidcError),

    #[error("repository error: {0}")]
    Repository(#[from] stitchd_db::RepositoryError),
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Builds an [`OidcProvider`] for the given [`AuthProviderId`] from DB.
///
/// Intended to be passed as the factory closure to
/// [`ProviderCache::get_or_build`].
pub struct OidcProviderFactory {
    repo: Arc<dyn AuthProviderRepository>,
    crypto: Arc<CryptoKey>,
}

impl OidcProviderFactory {
    #[must_use]
    pub fn new(repo: Arc<dyn AuthProviderRepository>, crypto: Arc<CryptoKey>) -> Self {
        Self { repo, crypto }
    }

    /// Build an [`OidcProvider`] for `provider_id`.
    ///
    /// 1. Loads the `AuthProvider` row from DB
    /// 2. Decrypts `client_secret_enc` with AES-256-GCM
    /// 3. Calls `OidcProvider::from_discovery`
    ///
    /// # Errors
    /// Returns [`FactoryError`] on DB miss, wrong type, decryption failure,
    /// or OIDC discovery failure.
    pub async fn build(&self, provider_id: AuthProviderId) -> Result<Arc<OidcProvider>, FactoryError> {
        let record = self
            .repo
            .find_by_id(provider_id)
            .await?
            .ok_or(FactoryError::NotFound(provider_id))?;

        if record.provider_type != ProviderType::Oidc {
            return Err(FactoryError::WrongType(provider_id));
        }

        let cfg: OidcConfig = serde_json::from_value(record.config.clone())
            .map_err(|e| FactoryError::InvalidConfig(e.to_string()))?;

        use base64::Engine as _;
        let enc_bytes = base64::engine::general_purpose::STANDARD
            .decode(&cfg.client_secret_enc)
            .map_err(|e| FactoryError::Decryption(format!("base64: {e}")))?;

        let secret_bytes = self
            .crypto
            .decrypt(&enc_bytes)
            .map_err(|e| FactoryError::Decryption(e.to_string()))?;

        let client_secret = String::from_utf8(secret_bytes)
            .map_err(|e| FactoryError::Decryption(format!("utf8: {e}")))?;

        let provider =
            OidcProvider::from_discovery(&cfg.issuer_url, &cfg.client_id, &client_secret).await?;

        Ok(Arc::new(provider))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use base64::Engine as _;
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
    // Helpers
    // -----------------------------------------------------------------------

    fn test_crypto() -> Arc<CryptoKey> {
        Arc::new(CryptoKey::from_bytes([0u8; 32]))
    }

    fn make_oidc_config(crypto: &CryptoKey, issuer_url: &str) -> serde_json::Value {
        let secret = b"test-secret";
        let enc = crypto.encrypt(secret).expect("encrypt should succeed");
        let enc_b64 = base64::engine::general_purpose::STANDARD.encode(&enc);
        serde_json::json!({
            "issuer_url": issuer_url,
            "client_id": "test-client-id",
            "client_secret_enc": enc_b64,
        })
    }

    fn make_provider(
        provider_type: ProviderType,
        config: serde_json::Value,
    ) -> AuthProvider {
        use chrono::Utc;
        AuthProvider {
            id: AuthProviderId::new(),
            org_id: OrganisationId::new(),
            provider_type,
            display_name: "Test".to_string(),
            config,
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn returns_not_found_when_provider_absent() {
        let repo = Arc::new(MockRepo { provider: None });
        let factory = OidcProviderFactory::new(repo, test_crypto());
        let id = AuthProviderId::new();
        let result: Result<_, FactoryError> = factory.build(id).await;
        assert!(matches!(result, Err(FactoryError::NotFound(_))));
    }

    #[tokio::test]
    async fn returns_wrong_type_for_saml_provider() {
        let crypto = test_crypto();
        let config = make_oidc_config(&crypto, "https://accounts.google.com");
        let provider = make_provider(ProviderType::Saml, config);
        let id = provider.id;
        let repo = Arc::new(MockRepo { provider: Some(provider) });
        let factory = OidcProviderFactory::new(repo, crypto);
        let result = factory.build(id).await;
        assert!(matches!(result, Err(FactoryError::WrongType(_))));
    }

    #[tokio::test]
    async fn returns_decryption_error_on_bad_base64() {
        let crypto = test_crypto();
        let config = serde_json::json!({
            "issuer_url": "https://accounts.google.com",
            "client_id": "id",
            "client_secret_enc": "!!!not-valid-base64!!!",
        });
        let provider = make_provider(ProviderType::Oidc, config);
        let id = provider.id;
        let repo = Arc::new(MockRepo { provider: Some(provider) });
        let factory = OidcProviderFactory::new(repo, crypto);
        let result = factory.build(id).await;
        assert!(matches!(result, Err(FactoryError::Decryption(_))));
    }

    #[tokio::test]
    async fn returns_decryption_error_on_wrong_key() {
        use base64::Engine as _;
        // Encrypt with key A, try to decrypt with key B
        let key_a = CryptoKey::from_bytes([0u8; 32]);
        let key_b = Arc::new(CryptoKey::from_bytes([1u8; 32]));

        let enc = key_a.encrypt(b"secret").unwrap();
        let enc_b64 = base64::engine::general_purpose::STANDARD.encode(&enc);
        let config = serde_json::json!({
            "issuer_url": "https://accounts.google.com",
            "client_id": "id",
            "client_secret_enc": enc_b64,
        });
        let provider = make_provider(ProviderType::Oidc, config);
        let id = provider.id;
        let repo = Arc::new(MockRepo { provider: Some(provider) });
        let factory = OidcProviderFactory::new(repo, key_b);
        let result = factory.build(id).await;
        assert!(matches!(result, Err(FactoryError::Decryption(_))));
    }

    #[tokio::test]
    async fn returns_invalid_config_on_missing_fields() {
        let config = serde_json::json!({ "some_random": "fields" });
        let provider = make_provider(ProviderType::Oidc, config);
        let id = provider.id;
        let repo = Arc::new(MockRepo { provider: Some(provider) });
        let factory = OidcProviderFactory::new(repo, test_crypto());
        let result = factory.build(id).await;
        assert!(matches!(result, Err(FactoryError::InvalidConfig(_))));
    }

    // Note: Testing actual OIDC discovery requires a live OIDC server.
    // The factory_calls_from_discovery_on_cache_miss test is covered by
    // integration tests using wiremock (see tests/oidc_integration.rs).
}
