# Spec: Org-Level OIDC & SAML with On-the-Fly Provider Instantiation

## Overview

Enable organisations to configure OIDC and SAML 2.0 authentication providers through
a management API. Provider integration layers (OIDC client, SAML processor) are built
lazily on the first login request and cached with a configurable TTL — no provider
config is pre-loaded at service startup.

The `auth_providers` DB table and `AuthProviderRepository` already exist. This track
wires up the gateway routes, gRPC handlers, on-the-fly instantiation layer, and TTL
cache that make org-level OIDC/SAML fully operational.

## Functional Requirements

### FR1: Provider Management API (Org Admin)
- `POST   /v1/orgs/{org_id}/auth-providers`          — register new OIDC or SAML provider
- `GET    /v1/orgs/{org_id}/auth-providers`          — list all providers for the org
- `GET    /v1/orgs/{org_id}/auth-providers/{id}`     — fetch single provider (secret redacted)
- `PUT    /v1/orgs/{org_id}/auth-providers/{id}`     — update config; evicts TTL cache entry
- `DELETE /v1/orgs/{org_id}/auth-providers/{id}`     — remove provider; enforces at-least-one-
                                                        enabled constraint; evicts cache

### FR2: OIDC Provider Config Fields
- `issuer_url`    — OIDC discovery endpoint (e.g. `https://accounts.google.com`)
- `client_id`     — OAuth2 client identifier
- `client_secret` — AES-256-GCM encrypted at rest
- `display_name`  — human-readable label
- `scopes`        — additional OAuth2 scopes beyond `openid email`

### FR3: SAML Provider Config Fields
- `idp_metadata_url` — URL to IdP XML metadata (fetched on-the-fly on cache miss)
- `idp_metadata_xml` — raw XML alternative (for air-gapped IdPs; mutually exclusive with URL)
- `name_id_format`   — e.g. `urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress`
- `display_name`     — human-readable label
- SP metadata endpoint: `GET /v1/orgs/{org_id}/auth-providers/{id}/saml/metadata`
  returns SP XML for the IdP admin to configure
- ACS URL returned in create response:
  `https://{host}/v1/auth/saml/{provider_id}/callback`

### FR4: Login Flow Entry Points

**OIDC:**
- `POST /v1/auth/oidc/{provider_id}/authorize`       — initiate for a specific provider
- `POST /v1/orgs/{org_id}/auth/oidc/authorize`       — initiate for org (first enabled OIDC)
- `GET  /v1/auth/oidc/{provider_id}/callback`        — exchange code → issue JWT + refresh token

**SAML:**
- `POST /v1/auth/saml/{provider_id}/sso`             — initiate SSO for specific provider
- `POST /v1/orgs/{org_id}/auth/saml/sso`             — initiate for org (first enabled SAML)
- `POST /v1/auth/saml/{provider_id}/callback`        — ACS: validate assertion → issue JWT

### FR5: On-the-Fly Provider Instantiation with TTL Cache
- No provider clients are created at service startup
- On first login request for a provider:
  1. Load config from DB
  2. Decrypt `client_secret` / fetch IdP metadata XML
  3. Build `OidcProvider` (via `from_discovery`) or SAML processor
  4. Store in in-memory cache keyed by `AuthProviderId`, TTL default 1 hour
- Cache eviction: immediate on `PUT` or `DELETE` of a provider
- On cache miss: rebuild from DB and restart TTL

### FR6: Security
- `client_secret` encrypted with AES-256-GCM (same mechanism as TOTP secrets)
- SAML assertion signatures validated against IdP certificate from metadata
- PKCE enforced on all OIDC flows
- CSRF state verified on OIDC callback
- Plaintext secrets never logged

## Non-Functional Requirements

- **NFR1:** OIDC discovery (network call) only on cache miss — not per login request
- **NFR2:** Cache TTL configurable via `PROVIDER_CACHE_TTL_SECS` env var (default: 3600)
- **NFR3:** Per-service-instance in-memory cache (no Redis needed for self-hosted)
- **NFR4:** ≥90% test coverage on new code (cargo-tarpaulin)
- **NFR5:** Encrypted config fields; plaintext never written to logs or audit trail

## Acceptance Criteria

- [ ] Org Admin can register, list, get, update, delete OIDC providers for their org
- [ ] Org Admin can register, list, get, update, delete SAML providers for their org
- [ ] OIDC login flow completes end-to-end: authorize → callback → JWT + refresh token issued
- [ ] SAML login flow completes end-to-end: SSO → ACS callback → JWT + refresh token issued
- [ ] Org-scoped login routes delegate to the first enabled provider of each type
- [ ] Provider clients are built lazily — no DB/network call at service startup
- [ ] TTL cache: second login within TTL reuses cached provider (no re-discovery)
- [ ] Cache evicted immediately on provider update or delete
- [ ] SP metadata endpoint returns valid SAML XML
- [ ] ACS URL returned in provider create response
- [ ] `client_secret` encrypted in DB; never returned in list/get responses

## Out of Scope

- JIT (just-in-time) user provisioning from SAML assertions
- SCIM provisioning
- Group/role claim mapping from IdP
- Multi-IdP selection UI (Admin UI is a separate project)
- Distributed provider cache (Redis)
- Client-side SDK auth via OIDC/SAML
