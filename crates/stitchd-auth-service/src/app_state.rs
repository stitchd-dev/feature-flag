//! Shared provider caches for the auth service.
//!
//! [`ProviderCaches`] holds one [`ProviderCache`] per provider protocol
//! (OIDC and SAML). Both caches are initialised empty at startup — no DB or
//! network calls are made until the first login request for each provider.

use std::{sync::Arc, time::Duration};

use stitchd_core::{
    auth::{OidcProvider, saml::SamlProvider},
    id::AuthProviderId,
};

use crate::provider_cache::ProviderCache;

/// Shared, lazily-populated provider caches.
///
/// Passed as `Arc<ProviderCaches>` into any service that needs to build or
/// evict provider instances (login handlers and provider management handlers).
pub struct ProviderCaches {
    pub oidc: ProviderCache<Arc<OidcProvider>>,
    pub saml: ProviderCache<Arc<SamlProvider>>,
}

impl ProviderCaches {
    /// Initialise both caches reading TTL from the environment.
    ///
    /// Zero providers are loaded; caches start empty.
    #[must_use]
    pub fn from_env() -> Self {
        let secs: u64 = std::env::var("PROVIDER_CACHE_TTL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3600);
        let ttl = Duration::from_secs(secs);
        Self {
            oidc: ProviderCache::new(ttl),
            saml: ProviderCache::new(ttl),
        }
    }

    /// Evict a provider from both caches simultaneously.
    ///
    /// Called on provider update or delete — whichever cache holds the entry
    /// is cleared; the other is a no-op.
    pub fn evict(&self, id: AuthProviderId) {
        self.oidc.evict(id);
        self.saml.evict(id);
    }
}
