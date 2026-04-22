# Implementation Plan: org_oidc_saml_20260422

## Phase 1: Provider Cache & On-the-Fly Instantiation Layer

- [x] Task 1: Write failing unit tests for `ProviderCache`
  <!-- files: crates/stitchd-auth-service/src/provider_cache.rs -->
  - [x] Sub-task: Test TTL expiry — entry returns None after TTL elapses
  - [x] Sub-task: Test cache hit — factory not called again within TTL
  - [x] Sub-task: Test `evict()` — entry removed immediately
  - [x] Sub-task: Test concurrent access (multiple tokio tasks racing on same provider_id)
- [x] Task 2: Implement `ProviderCache` in `stitchd-auth-service`
  <!-- files: crates/stitchd-auth-service/src/provider_cache.rs -->
  <!-- depends: task1 -->
  - [x] Sub-task: `DashMap<AuthProviderId, CacheEntry>` where `CacheEntry` holds built provider + expiry `Instant`
  - [x] Sub-task: `get_or_build(id, factory)` async — check TTL, call factory on miss, insert
  - [x] Sub-task: `evict(id)` — immediate removal
  - [x] Sub-task: `PROVIDER_CACHE_TTL_SECS` env var, default 3600
- [x] Task 3: Write failing tests for `OidcProviderFactory`, then implement
  <!-- files: crates/stitchd-auth-service/src/oidc_factory.rs -->
  <!-- depends: task2 -->
  - [x] Sub-task: Mock DB — verify `OidcProvider::from_discovery` called on cache miss only
  - [x] Sub-task: Load `AuthProvider` from DB via `AuthProviderRepository::find_by_id`
  - [x] Sub-task: Decrypt `client_secret` (existing AES-256-GCM path)
  - [x] Sub-task: Call `OidcProvider::from_discovery(issuer_url, client_id, decrypted_secret)`
- [x] Task 4: Write failing tests for `SamlProviderFactory`, then implement
  <!-- files: crates/stitchd-auth-service/src/saml_factory.rs -->
  <!-- depends: task2 -->
  - [x] Sub-task: Test URL fetch path — mock HTTP, verify metadata parsed correctly
  - [x] Sub-task: Test raw XML path — bypass HTTP, parse directly
  - [x] Sub-task: Load config from DB; branch on `idp_metadata_url` vs `idp_metadata_xml`
  - [x] Sub-task: Fetch XML via `reqwest` if URL provided
  - [x] Sub-task: Parse IdP metadata (SSO URL, certificate) via `stitchd-core::auth::saml`
- [x] Task 5: Wire `ProviderCache` onto auth-service `AppState`
  <!-- files: crates/stitchd-auth-service/src/lib.rs, crates/stitchd-auth-service/src/main.rs -->
  <!-- depends: task3, task4 -->
  - [x] Sub-task: Add `provider_cache: Arc<ProviderCache>` to `AppState`
  - [x] Sub-task: Initialise in `main.rs` — zero providers loaded; no DB/network at startup
- [ ] Task: Conductor - User Manual Verification 'Provider Cache & On-the-Fly Instantiation Layer' (Protocol in workflow.md)

## Phase 2: Provider Management gRPC Handlers & Gateway Routes

- [ ] Task 1: Write failing tests for provider management gRPC handlers
  <!-- files: crates/stitchd-auth-service/src/management.rs -->
  - [ ] Sub-task: `CreateProvider` OIDC — encrypts secret, persists, returns with ACS URL for SAML
  - [ ] Sub-task: `CreateProvider` SAML — validates metadata URL or XML before persist
  - [ ] Sub-task: `ListProviders` — returns correct org's providers; secrets absent
  - [ ] Sub-task: `GetProvider` — secret field redacted
  - [ ] Sub-task: `UpdateProvider` — repo updated, cache entry evicted
  - [ ] Sub-task: `DeleteProvider` — last-enabled constraint enforced; cache evicted
- [ ] Task 2: Add proto messages for provider management
  <!-- files: proto/auth/v1/management.proto, crates/stitchd-proto/src/lib.rs -->
  - [ ] Sub-task: `OidcConfig`, `SamlConfig` config message types
  - [ ] Sub-task: `AuthProviderResponse` (config redacted variant)
  - [ ] Sub-task: `CreateAuthProviderRequest/Response`, `List/Get/Update/Delete` variants
  - [ ] Sub-task: `GetSamlSpMetadataRequest/Response`
  - [ ] Sub-task: Regenerate prost bindings (`cargo build -p stitchd-proto`)
- [ ] Task 3: Implement gRPC handlers in `management.rs`
  <!-- files: crates/stitchd-auth-service/src/management.rs -->
  <!-- depends: task1, task2 -->
  - [ ] Sub-task: `create_auth_provider` — encrypt secret, repo create, return ACS URL
  - [ ] Sub-task: `list_auth_providers` + `get_auth_provider` — repo read, redact secrets
  - [ ] Sub-task: `update_auth_provider` — repo update + `cache.evict(id)`
  - [ ] Sub-task: `delete_auth_provider` — repo delete + `cache.evict(id)`
  - [ ] Sub-task: `get_saml_sp_metadata` — load provider from cache, emit SP XML
- [ ] Task 4: Add gateway REST routes for provider management
  <!-- files: crates/stitchd-gateway/src/routes/auth_providers.rs -->
  <!-- depends: task3 -->
  - [ ] Sub-task: `POST   /v1/orgs/{org_id}/auth-providers`
  - [ ] Sub-task: `GET    /v1/orgs/{org_id}/auth-providers`
  - [ ] Sub-task: `GET    /v1/orgs/{org_id}/auth-providers/{id}`
  - [ ] Sub-task: `PUT    /v1/orgs/{org_id}/auth-providers/{id}`
  - [ ] Sub-task: `DELETE /v1/orgs/{org_id}/auth-providers/{id}`
  - [ ] Sub-task: `GET    /v1/orgs/{org_id}/auth-providers/{id}/saml/metadata`
  - [ ] Sub-task: RBAC guard — Org Admin required for all management routes
  - [ ] Sub-task: utoipa OpenAPI annotations
- [ ] Task: Conductor - User Manual Verification 'Provider Management gRPC Handlers & Gateway Routes' (Protocol in workflow.md)

## Phase 3: OIDC Login Flow
<!-- depends: phase2 -->
<!-- execution: parallel -->

- [ ] Task 1: Write failing tests for OIDC login handlers
  <!-- files: crates/stitchd-auth-service/src/oidc_login.rs -->
  - [ ] Sub-task: `OidcAuthorize` (provider-scoped) — returns redirect URL; CSRF state stored
  - [ ] Sub-task: `OidcAuthorize` (org-scoped) — picks first enabled OIDC provider
  - [ ] Sub-task: `OidcCallback` valid — code exchange issues JWT + refresh token
  - [ ] Sub-task: `OidcCallback` invalid CSRF state — rejected
  - [ ] Sub-task: `OidcCallback` expired state — rejected
- [ ] Task 2: Add proto messages for OIDC login
  <!-- files: proto/auth/v1/oidc_login.proto, crates/stitchd-proto/src/lib.rs -->
  - [ ] Sub-task: `OidcAuthorizeRequest` (provider_id xor org_id, redirect_uri)
  - [ ] Sub-task: `OidcAuthorizeResponse` (redirect_url)
  - [ ] Sub-task: `OidcCallbackRequest` (provider_id, code, state)
  - [ ] Sub-task: `OidcCallbackResponse` (access_token, refresh_token, expires_in, user_id, org_id)
  - [ ] Sub-task: Regenerate prost bindings
- [ ] Task 3: Implement OIDC pending-state store
  <!-- files: crates/stitchd-auth-service/src/oidc_state.rs -->
  - [ ] Sub-task: `DashMap<String, OidcPendingState>` (pkce_verifier, provider_id, expiry)
  - [ ] Sub-task: 5-minute TTL; expired entries rejected at callback
- [ ] Task 4: Implement OIDC authorize + callback handlers
  <!-- files: crates/stitchd-auth-service/src/oidc_login.rs -->
  <!-- depends: task1, task2, task3 -->
  - [ ] Sub-task: Load `OidcProvider` from `ProviderCache`
  - [ ] Sub-task: Call `provider.authorization_url(redirect_uri)` → (url, verifier, state)
  - [ ] Sub-task: Store `OidcPendingState` keyed on state; org-scoped variant
  - [ ] Sub-task: Callback: validate + consume state; `exchange_code`; find-or-create user; issue JWT
- [ ] Task 5: Add gateway REST routes for OIDC
  <!-- files: crates/stitchd-gateway/src/routes/oidc.rs -->
  <!-- depends: task4 -->
  - [ ] Sub-task: `POST /v1/auth/oidc/{provider_id}/authorize`
  - [ ] Sub-task: `POST /v1/orgs/{org_id}/auth/oidc/authorize`
  - [ ] Sub-task: `GET  /v1/auth/oidc/{provider_id}/callback`
  - [ ] Sub-task: utoipa OpenAPI annotations
- [ ] Task: Conductor - User Manual Verification 'OIDC Login Flow' (Protocol in workflow.md)

## Phase 4: SAML Login Flow
<!-- depends: phase2 -->
<!-- execution: parallel -->

- [ ] Task 1: Write failing tests for SAML login handlers
  <!-- files: crates/stitchd-auth-service/src/saml_login.rs -->
  - [ ] Sub-task: `SamlSsoInitiate` (provider-scoped) — returns IdP redirect URL
  - [ ] Sub-task: `SamlSsoInitiate` (org-scoped) — picks first enabled SAML provider
  - [ ] Sub-task: `SamlAcsCallback` valid — signed assertion issues JWT + refresh token
  - [ ] Sub-task: `SamlAcsCallback` invalid signature — rejected
  - [ ] Sub-task: `SamlAcsCallback` replayed RelayState — rejected
- [ ] Task 2: Add proto messages for SAML login
  <!-- files: proto/auth/v1/saml_login.proto, crates/stitchd-proto/src/lib.rs -->
  - [ ] Sub-task: `SamlSsoRequest` (provider_id xor org_id), `SamlSsoResponse` (redirect_url, relay_state)
  - [ ] Sub-task: `SamlAcsRequest` (provider_id, saml_response_b64, relay_state)
  - [ ] Sub-task: `SamlAcsResponse` (access_token, refresh_token, expires_in, user_id, org_id)
  - [ ] Sub-task: Regenerate prost bindings
- [ ] Task 3: Implement SAML RelayState store
  <!-- files: crates/stitchd-auth-service/src/saml_state.rs -->
  - [ ] Sub-task: `DashMap<String, SamlPendingState>` (provider_id, expiry); 10-minute TTL
- [ ] Task 4: Implement SAML SSO initiation + ACS callback handlers
  <!-- files: crates/stitchd-auth-service/src/saml_login.rs -->
  <!-- depends: task1, task2, task3 -->
  - [ ] Sub-task: Load SAML processor from `ProviderCache`
  - [ ] Sub-task: Generate AuthnRequest XML; deflate + base64; build IdP redirect URL; store RelayState
  - [ ] Sub-task: Org-scoped SSO variant
  - [ ] Sub-task: ACS: validate RelayState; decode SAMLResponse; verify signature; extract NameID
  - [ ] Sub-task: Find-or-create user by email; issue JWT + refresh token
- [ ] Task 5: Add gateway REST routes for SAML
  <!-- files: crates/stitchd-gateway/src/routes/saml.rs -->
  <!-- depends: task4 -->
  - [ ] Sub-task: `POST /v1/auth/saml/{provider_id}/sso`
  - [ ] Sub-task: `POST /v1/orgs/{org_id}/auth/saml/sso`
  - [ ] Sub-task: `POST /v1/auth/saml/{provider_id}/callback` (ACS — form-encoded body)
  - [ ] Sub-task: utoipa OpenAPI annotations
- [ ] Task: Conductor - User Manual Verification 'SAML Login Flow' (Protocol in workflow.md)

## Phase 5: Integration Tests & Coverage
<!-- depends: phase3, phase4 -->

- [ ] Task 1: OIDC integration tests (mocked IdP)
  <!-- files: crates/stitchd-auth-service/tests/oidc_integration.rs -->
  - [ ] Sub-task: Use `wiremock` to mock OIDC discovery + token endpoints
  - [ ] Sub-task: Full flow: create provider → authorize → mock callback → JWT verified
  - [ ] Sub-task: Assert cache hit on second authorize (no re-discovery)
- [ ] Task 2: SAML integration tests (mocked IdP)
  <!-- files: crates/stitchd-auth-service/tests/saml_integration.rs -->
  - [ ] Sub-task: Mock IdP metadata XML (inline fixture)
  - [ ] Sub-task: Full flow: create provider → SSO initiate → mock signed ACS → JWT verified
- [ ] Task 3: Coverage gate
  <!-- files: — -->
  <!-- depends: task1, task2 -->
  - [ ] Sub-task: `cargo tarpaulin -p stitchd-auth-service` — verify ≥90%; fix gaps
  - [ ] Sub-task: `cargo tarpaulin -p stitchd-gateway` — verify ≥90%; fix gaps
- [ ] Task 4: Final quality gate
  <!-- files: — -->
  <!-- depends: task3 -->
  - [ ] Sub-task: `cargo fmt --all --check`
  - [ ] Sub-task: `cargo clippy --workspace --all-targets -- -D warnings`
  - [ ] Sub-task: `SQLX_OFFLINE=false cargo sqlx prepare --workspace` (if new sqlx queries added)
- [ ] Task: Conductor - User Manual Verification 'Integration Tests & Coverage' (Protocol in workflow.md)
