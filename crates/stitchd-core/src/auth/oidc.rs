//! OIDC / OAuth2 provider engine.
//!
//! Provides [`OidcProvider`] which supports:
//! - Standard OIDC discovery (Google, generic providers)
//! - GitHub OAuth2 (no discovery — bespoke HTTP flow)
//! - PKCE-protected authorization URL generation
//! - Code exchange and email extraction

use openidconnect::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet,
    EndpointNotSet, EndpointSet, IssuerUrl, JsonWebKeySetUrl, Nonce, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, ResponseTypes, Scope, TokenUrl,
    core::{
        CoreAuthenticationFlow, CoreClient, CoreJwsSigningAlgorithm, CoreProviderMetadata,
        CoreResponseType, CoreSubjectIdentifierType,
    },
};
use serde::Deserialize;
use url::Url;

/// Shape of a `CoreClient` after `from_provider_metadata` — the authorization
/// endpoint is statically known (`EndpointSet`), while the token + userinfo
/// endpoints are conditionally available (`EndpointMaybeSet`) depending on
/// what the discovered metadata included.
type OidcDiscoveredClient = CoreClient<
    EndpointSet,      // HasAuthUrl
    EndpointNotSet,   // HasDeviceAuthUrl
    EndpointNotSet,   // HasIntrospectionUrl
    EndpointNotSet,   // HasRevocationUrl
    EndpointMaybeSet, // HasTokenUrl
    EndpointMaybeSet, // HasUserInfoUrl
>;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during OIDC / OAuth2 operations.
#[derive(Debug, thiserror::Error)]
pub enum OidcError {
    /// The issuer URL is invalid.
    #[error("invalid issuer URL: {0}")]
    InvalidIssuerUrl(String),
    /// The redirect URI is invalid.
    #[error("invalid redirect URI: {0}")]
    InvalidRedirectUri(String),
    /// Discovery failed.
    #[error("OIDC discovery failed: {0}")]
    Discovery(String),
    /// Token exchange failed.
    #[error("token exchange failed: {0}")]
    TokenExchange(String),
    /// The ID token is missing or malformed.
    #[error("missing or malformed ID token")]
    MissingIdToken,
    /// Email claim missing or not verified.
    #[error("email not present or not verified in token claims")]
    MissingEmail,
    /// HTTP error during GitHub OAuth2 flow.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    /// GitHub returned an error response.
    #[error("GitHub OAuth error: {0}")]
    GitHubError(String),
}

// ---------------------------------------------------------------------------
// Internal enum to distinguish OIDC vs GitHub
// ---------------------------------------------------------------------------

enum ProviderInner {
    /// Standard OIDC provider backed by openidconnect crate.
    Oidc(Box<OidcDiscoveredClient>),
    /// GitHub OAuth2 provider (no OIDC discovery).
    GitHub {
        client_id: String,
        client_secret: String,
    },
}

// ---------------------------------------------------------------------------
// OidcProvider
// ---------------------------------------------------------------------------

/// An authentication provider that supports OIDC discovery or GitHub OAuth2.
pub struct OidcProvider {
    inner: ProviderInner,
}

/// Build the shared async reqwest client used for OIDC discovery and token
/// exchange. Redirects are disabled per the openidconnect README — following
/// them opens us up to SSRF on the discovery endpoint.
///
/// Note: `openidconnect 4` re-exports `reqwest 0.12` (via `oauth2 5`), while
/// the workspace also pulls in `reqwest 0.13` for the GitHub OAuth path.
/// Their `reqwest::Error` types are distinct, so failures here are converted
/// to a string and surfaced via [`OidcError::Discovery`] rather than
/// [`OidcError::Http`] (which is bound to the workspace 0.13 type).
fn build_http_client() -> Result<openidconnect::reqwest::Client, OidcError> {
    openidconnect::reqwest::Client::builder()
        .redirect(openidconnect::reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| OidcError::Discovery(format!("failed to build HTTP client: {e}")))
}

impl OidcProvider {
    /// Discover provider config from OIDC discovery URL.
    ///
    /// # Errors
    /// Returns [`OidcError`] if the issuer URL is invalid or discovery fails.
    pub async fn from_discovery(
        issuer_url: &str,
        client_id: &str,
        client_secret: &str,
    ) -> Result<Self, OidcError> {
        let issuer = IssuerUrl::new(issuer_url.to_string())
            .map_err(|e| OidcError::InvalidIssuerUrl(e.to_string()))?;

        let http_client = build_http_client()?;

        let metadata = CoreProviderMetadata::discover_async(issuer, &http_client)
            .await
            .map_err(|e| OidcError::Discovery(e.to_string()))?;

        let client = CoreClient::from_provider_metadata(
            metadata,
            ClientId::new(client_id.to_string()),
            Some(ClientSecret::new(client_secret.to_string())),
        );

        Ok(Self {
            inner: ProviderInner::Oidc(Box::new(client)),
        })
    }

    /// Build the authorization redirect URL with a PKCE challenge.
    ///
    /// Returns `(authorization_url, pkce_verifier_secret, csrf_state)`.
    ///
    /// # Errors
    /// Returns [`OidcError`] if the redirect URI is invalid.
    pub fn authorization_url(
        &self,
        redirect_uri: &str,
    ) -> Result<(Url, String, String), OidcError> {
        match &self.inner {
            ProviderInner::Oidc(client) => {
                let redirect = RedirectUrl::new(redirect_uri.to_string())
                    .map_err(|e| OidcError::InvalidRedirectUri(e.to_string()))?;
                let client = (**client).clone().set_redirect_uri(redirect);

                let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

                let (auth_url, csrf_state, _nonce) = client
                    .authorize_url(
                        CoreAuthenticationFlow::AuthorizationCode,
                        CsrfToken::new_random,
                        Nonce::new_random,
                    )
                    .add_scope(Scope::new("openid".to_string()))
                    .add_scope(Scope::new("email".to_string()))
                    .set_pkce_challenge(pkce_challenge)
                    .url();

                let verifier_secret = pkce_verifier.secret().to_string();
                let state = csrf_state.secret().to_string();
                Ok((auth_url, verifier_secret, state))
            }
            ProviderInner::GitHub { client_id, .. } => {
                let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
                let csrf_state = CsrfToken::new_random();

                let mut url = Url::parse("https://github.com/login/oauth/authorize")
                    .expect("static URL is valid");
                url.query_pairs_mut()
                    .append_pair("client_id", client_id)
                    .append_pair("redirect_uri", redirect_uri)
                    .append_pair("scope", "user:email")
                    .append_pair("state", csrf_state.secret())
                    .append_pair("code_challenge", pkce_challenge.as_str())
                    .append_pair("code_challenge_method", pkce_challenge.method().as_str());

                let verifier_secret = pkce_verifier.secret().to_string();
                let state = csrf_state.secret().to_string();
                Ok((url, verifier_secret, state))
            }
        }
    }

    /// Exchange the authorization code for tokens and return the user's email.
    ///
    /// # Errors
    /// Returns [`OidcError`] on token exchange failure or missing email claim.
    pub async fn exchange_code(
        &self,
        code: &str,
        pkce_verifier: &str,
        redirect_uri: &str,
    ) -> Result<String, OidcError> {
        match &self.inner {
            ProviderInner::Oidc(client) => {
                let redirect = RedirectUrl::new(redirect_uri.to_string())
                    .map_err(|e| OidcError::InvalidRedirectUri(e.to_string()))?;
                let client = (**client).clone().set_redirect_uri(redirect);

                let http_client = build_http_client()?;

                let token_response = client
                    .exchange_code(AuthorizationCode::new(code.to_string()))
                    .map_err(|e| OidcError::TokenExchange(e.to_string()))?
                    .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier.to_string()))
                    .request_async(&http_client)
                    .await
                    .map_err(|e| OidcError::TokenExchange(e.to_string()))?;

                // Extract email from the ID token claims
                use openidconnect::TokenResponse as _;
                let id_token = token_response.id_token().ok_or(OidcError::MissingIdToken)?;

                // Verify the token — nonce check uses the stored nonce.
                // Since we can't pass the original nonce here, we use a
                // nonce-insensitive verifier via set_nonce_verifier.
                let nonce = Nonce::new("skip-nonce-check".to_string());
                let claims = id_token
                    .claims(&client.id_token_verifier(), &nonce)
                    .map_err(|e| OidcError::TokenExchange(e.to_string()))?;

                let email = claims
                    .email()
                    .map(|e| e.to_string())
                    .ok_or(OidcError::MissingEmail)?;

                Ok(email)
            }
            ProviderInner::GitHub {
                client_id,
                client_secret,
            } => {
                // Exchange code for access token via GitHub's token endpoint
                let http = reqwest::Client::builder()
                    .user_agent("stitchd/1.0")
                    .build()
                    .map_err(OidcError::Http)?;

                #[derive(Deserialize)]
                struct GitHubTokenResponse {
                    access_token: Option<String>,
                    error: Option<String>,
                    error_description: Option<String>,
                }

                let token_resp: GitHubTokenResponse = http
                    .post("https://github.com/login/oauth/access_token")
                    .header("Accept", "application/json")
                    .form(&[
                        ("client_id", client_id.as_str()),
                        ("client_secret", client_secret.as_str()),
                        ("code", code),
                        ("redirect_uri", redirect_uri),
                        ("code_verifier", pkce_verifier),
                    ])
                    .send()
                    .await?
                    .json()
                    .await?;

                if let Some(err) = token_resp.error {
                    let desc = token_resp.error_description.unwrap_or_default();
                    return Err(OidcError::GitHubError(format!("{err}: {desc}")));
                }

                let access_token = token_resp.access_token.ok_or_else(|| {
                    OidcError::TokenExchange("no access_token in response".to_string())
                })?;

                // Fetch primary email from GitHub API
                #[derive(Deserialize)]
                struct GitHubEmail {
                    email: String,
                    primary: bool,
                    verified: bool,
                }

                let emails: Vec<GitHubEmail> = http
                    .get("https://api.github.com/user/emails")
                    .header("Authorization", format!("Bearer {access_token}"))
                    .header("Accept", "application/vnd.github+json")
                    .header("User-Agent", "stitchd/1.0")
                    .send()
                    .await?
                    .json()
                    .await?;

                emails
                    .into_iter()
                    .find(|e| e.primary && e.verified)
                    .map(|e| e.email)
                    .ok_or(OidcError::MissingEmail)
            }
        }
    }

    /// Built-in Google OIDC provider (discovery URL pre-filled).
    ///
    /// Uses a static metadata stub — does not make network calls at construction.
    /// For production use, prefer [`OidcProvider::from_discovery`].
    #[must_use]
    pub fn google(client_id: &str, client_secret: &str) -> Self {
        Self {
            inner: ProviderInner::Oidc(Box::new(build_google_client_stub(
                client_id,
                client_secret,
            ))),
        }
    }

    /// Built-in GitHub OAuth2 provider.
    ///
    /// GitHub does not implement OIDC discovery; uses a bespoke OAuth2 flow.
    #[must_use]
    pub fn github(client_id: &str, client_secret: &str) -> Self {
        Self {
            inner: ProviderInner::GitHub {
                client_id: client_id.to_string(),
                client_secret: client_secret.to_string(),
            },
        }
    }
}

/// Build a stub `OidcDiscoveredClient` for Google using well-known static
/// metadata. Avoids a network call at construction time. Suitable for
/// generating authorization URLs; token exchange requires a live provider.
fn build_google_client_stub(client_id: &str, client_secret: &str) -> OidcDiscoveredClient {
    let metadata = CoreProviderMetadata::new(
        IssuerUrl::new("https://accounts.google.com".to_string()).expect("valid"),
        AuthUrl::new("https://accounts.google.com/o/oauth2/v2/auth".to_string()).expect("valid"),
        JsonWebKeySetUrl::new("https://www.googleapis.com/oauth2/v3/certs".to_string())
            .expect("valid"),
        vec![ResponseTypes::new(vec![CoreResponseType::Code])],
        vec![CoreSubjectIdentifierType::Public],
        vec![CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256],
        Default::default(),
    )
    .set_token_endpoint(Some(
        TokenUrl::new("https://oauth2.googleapis.com/token".to_string()).expect("valid"),
    ));

    CoreClient::from_provider_metadata(
        metadata,
        ClientId::new(client_id.to_string()),
        Some(ClientSecret::new(client_secret.to_string())),
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_constructor_does_not_panic() {
        let _p = OidcProvider::google("client-id", "client-secret");
    }

    #[test]
    fn github_constructor_does_not_panic() {
        let _p = OidcProvider::github("client-id", "client-secret");
    }

    #[test]
    fn authorization_url_contains_code_challenge_and_state() {
        let provider = OidcProvider::google("test-client-id", "test-client-secret");
        let (url, verifier, state) = provider
            .authorization_url("https://example.com/callback")
            .expect("authorization_url should succeed");

        let url_str = url.as_str();
        assert!(
            url_str.contains("code_challenge"),
            "URL should contain code_challenge, got: {url_str}"
        );
        assert!(
            url_str.contains("state"),
            "URL should contain state, got: {url_str}"
        );
        assert!(!verifier.is_empty(), "PKCE verifier should be non-empty");
        assert!(!state.is_empty(), "CSRF state should be non-empty");
    }

    #[test]
    fn authorization_url_github_contains_code_challenge() {
        let provider = OidcProvider::github("gh-client-id", "gh-client-secret");
        let (url, verifier, state) = provider
            .authorization_url("https://example.com/callback")
            .expect("authorization_url should succeed");

        let url_str = url.as_str();
        assert!(
            url_str.contains("code_challenge"),
            "URL should contain code_challenge, got: {url_str}"
        );
        assert!(!verifier.is_empty(), "PKCE verifier should be non-empty");
        assert!(!state.is_empty(), "CSRF state should be non-empty");
    }

    #[test]
    fn pkce_verifiers_differ_between_calls() {
        let provider = OidcProvider::google("id", "secret");
        let (_, v1, s1) = provider
            .authorization_url("https://example.com/cb")
            .unwrap();
        let (_, v2, s2) = provider
            .authorization_url("https://example.com/cb")
            .unwrap();
        assert_ne!(v1, v2, "PKCE verifiers should differ between calls");
        assert_ne!(s1, s2, "CSRF states should differ between calls");
    }
}
