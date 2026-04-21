# Plan: JWT / Multi-Mechanism Human Auth
Track: auth_20260421

## Phase 1: Database Schema & Platform Identity Model
<!-- execution: parallel -->

- [x] Task 1: PostgreSQL migrations — b07de50
  <!-- files: crates/stitchd-db/migrations/ -->
  - [x] Sub-task: Write failing test asserting all new tables and columns exist post-migration
  - [x] Sub-task: `users` table (platform-level): id, email UNIQUE, display_name,
        avatar_url, password_hash, token_secret UUID, totp_secret bytea (nullable,
        encrypted), totp_enabled, status (active|deactivated), created_at, updated_at
  - [x] Sub-task: `org_memberships`: user_id, org_id, role, joined_at; UNIQUE(user_id, org_id)
  - [x] Sub-task: `user_project_roles`: user_id, project_id, role; UNIQUE(user_id, project_id)
  - [x] Sub-task: `user_env_roles`: user_id, env_id, role; UNIQUE(user_id, env_id)
  - [x] Sub-task: `refresh_tokens`: id, user_id, org_id, token_hash, device_hint,
        issued_at, expires_at, revoked_at, last_used_at;
        INDEX(user_id, revoked_at, expires_at)
  - [x] Sub-task: `auth_providers`: id, org_id, provider_type (password|oidc|saml),
        display_name, config JSONB, enabled, created_at, updated_at
  - [x] Sub-task: `invites`: id, org_id, email, org_role, invited_by_user_id,
        token_hash, expires_at (72h), accepted_at
  - [x] Sub-task: `mfa_challenges`: id, user_id, challenge_token_hash, expires_at, used_at
  - [x] Sub-task: `mfa_recovery_codes`: id, user_id, code_hash, used_at
  - [x] Sub-task: `password_reset_otps`: id, email, otp_hash, expires_at, used_at
  - [x] Sub-task: Pass all migration tests

- [x] Task 2: Domain types in `stitchd-core` — 945d29a
  <!-- files: crates/stitchd-core/src/auth/ -->
  - [x] Sub-task: Write failing tests for enum parsing, role ordering, newtype serde
  - [x] Sub-task: `UserId`, `AuthProviderId`, `RefreshTokenId`, `InviteId`, `MfaChallengeId`
        newtypes (macro_rules! with sqlx::Type transparent)
  - [x] Sub-task: `ProviderType` enum (Password|Oidc|Saml)
  - [x] Sub-task: `OrgRole` enum (OrgAdmin|OrgMember) with PartialOrd
  - [x] Sub-task: `ProjectRole` enum (ProjectAdmin|ProjectViewer) with PartialOrd
  - [x] Sub-task: `EnvRole` enum (EnvPublisher|EnvViewer) with PartialOrd
  - [x] Sub-task: `UserStatus` enum (Active|Deactivated)
  - [x] Sub-task: `User`, `OrgMembership`, `RefreshToken`, `AuthProvider`, `Invite` structs
  - [x] Sub-task: Pass all tests

- [x] Task: Conductor - User Manual Verification 'Phase 1' (Protocol in workflow.md)

## Phase 2: Core Token & Crypto Engine
<!-- depends: phase1 -->

- [x] Task 1: Crypto primitives (`stitchd-core`) — 60edb1c
  <!-- files: crates/stitchd-core/src/auth/crypto.rs -->
  - [x] Sub-task: Write failing tests for AES encrypt/decrypt round-trip, Argon2id
        hash+verify, secure random generation
  - [x] Sub-task: Update tech-stack.md — add aes-gcm, argon2, rand crates
  - [x] Sub-task: AES-256-GCM encrypt/decrypt (aes-gcm crate); `CryptoKey` loaded
        from `AUTH_ENCRYPTION_KEY` env var (32 bytes, base64)
  - [x] Sub-task: Argon2id hash + verify for passwords and recovery codes
  - [x] Sub-task: `generate_opaque_token() -> (raw: String, hash: String)` (32 random bytes, hex)
  - [x] Sub-task: `generate_otp() -> (code: String, hash: String)` (6-digit numeric)
  - [x] Sub-task: Pass all tests

- [x] Task 2: JWT engine (`stitchd-core`) — ed3956b
  <!-- files: crates/stitchd-core/src/auth/jwt.rs -->
  - [x] Sub-task: Write failing tests: issue+verify round-trip, expiry rejection,
        wrong-secret rejection, stale token after secret rotation
  - [x] Sub-task: Update tech-stack.md — add jsonwebtoken crate
  - [x] Sub-task: `AccessTokenClaims` struct (user_id, org_id, email, org_role, exp, iat)
  - [x] Sub-task: `JwtEngine::issue(user_id, org_id, email, org_role, token_secret) -> String`
  - [x] Sub-task: `JwtEngine::verify(token, token_secret) -> Result<AccessTokenClaims>`
  - [x] Sub-task: Pass all tests

- [x] Task 3: Refresh token repository (`stitchd-db`) — 00b7bec
  <!-- files: crates/stitchd-db/src/auth/refresh_tokens.rs -->
  <!-- depends: task1 -->
  - [x] Sub-task: Write failing integration tests (`#[sqlx::test]`)
  - [x] Sub-task: `create(user_id, org_id, device_hint, ttl_days) -> (RefreshToken, raw_token)`
  - [x] Sub-task: `find_by_hash(hash) -> Option<RefreshToken>`
  - [x] Sub-task: `consume(id) -> Option<RefreshToken>` (sets revoked_at; used for rotation)
  - [x] Sub-task: `revoke(id)` — manual revocation
  - [x] Sub-task: `revoke_all_for_user(user_id)` — sign-out-all
  - [x] Sub-task: `list_active(user_id) -> Vec<RefreshToken>`
  - [x] Sub-task: Pass all tests

- [x] Task 4: Axum auth middleware (`stitchd-server`) — cdbf59f
  <!-- files: crates/stitchd-server/src/api/auth/middleware.rs -->
  <!-- depends: task2, task3 -->
  - [x] Sub-task: Write failing tests (valid token, expired, wrong secret, missing
        header, deactivated user, insufficient org role, insufficient project role)
  - [x] Sub-task: `AuthLayer`: extracts Bearer, calls `JwtEngine::verify` (fetches
        `token_secret` from user repo), injects `AuthenticatedUser` into extensions
  - [x] Sub-task: 401 on missing/invalid token; 403 on deactivated user
  - [x] Sub-task: `RequireOrgRole(OrgRole)` extractor
  - [x] Sub-task: `RequireProjectRole(ProjectRole)` extractor (queries `user_project_roles`)
  - [x] Sub-task: `RequireEnvRole(EnvRole)` extractor (queries `user_env_roles`)
  - [x] Sub-task: Pass all tests

- [x] Task: Conductor - User Manual Verification 'Phase 2' (Protocol in workflow.md)

## Phase 3: Password Auth + Session Management
<!-- depends: phase2 -->

- [x] Task 1: User repository (`stitchd-db`) — 251fa65
  <!-- files: crates/stitchd-db/src/auth/users.rs -->
  - [x] Sub-task: Write failing integration tests
  - [x] Sub-task: `create(email, display_name, password_hash) -> User`
  - [x] Sub-task: `find_by_email(email) -> Option<User>`
  - [x] Sub-task: `find_by_id(id) -> Option<User>`
  - [x] Sub-task: `rotate_token_secret(user_id) -> Uuid`
  - [x] Sub-task: `update_status(user_id, status)`
  - [x] Sub-task: `update_password_hash(user_id, hash)`
  - [x] Sub-task: `update_profile(user_id, display_name, avatar_url)`
  - [x] Sub-task: Pass all tests

- [x] Task 2: Org membership repository (`stitchd-db`) — 251fa65
  <!-- files: crates/stitchd-db/src/auth/memberships.rs -->
  - [x] Sub-task: Write failing integration tests
  - [x] Sub-task: `add_member(user_id, org_id, role) -> OrgMembership`
  - [x] Sub-task: `find_membership(user_id, org_id) -> Option<OrgMembership>`
  - [x] Sub-task: `list_orgs_for_user(user_id) -> Vec<OrgMembership>`
  - [x] Sub-task: `remove_member(user_id, org_id)`
  - [x] Sub-task: `update_role(user_id, org_id, role)`
  - [x] Sub-task: Pass all tests

- [x] Task 3: Password auth + session handlers (`stitchd-server`) — 617604f
  <!-- files: crates/stitchd-server/src/api/auth/password.rs,
              crates/stitchd-server/src/api/auth/sessions.rs -->
  <!-- depends: task1, task2 -->
  - [x] Sub-task: Write failing handler tests (login ok, wrong password, deactivated,
        mfa_required branch, refresh rotation, sign-out-all, org-switch)
  - [x] Sub-task: POST /auth/login — verify Argon2id → if MFA disabled issue tokens;
        if MFA enabled return `{mfa_required:true, challenge_token}`
  - [x] Sub-task: POST /auth/refresh — find+consume refresh token → issue new pair
  - [x] Sub-task: POST /auth/logout — revoke current refresh token
  - [x] Sub-task: DELETE /auth/sessions — rotate token_secret + revoke_all
  - [x] Sub-task: GET /auth/sessions — list active refresh tokens for caller
  - [x] Sub-task: DELETE /auth/sessions/{id} — revoke specific session
  - [x] Sub-task: POST /auth/switch-org {org_id} — validate membership → new pair
  - [x] Sub-task: Pass all tests

- [ ] Task: Conductor - User Manual Verification 'Phase 3' (Protocol in workflow.md)

## Phase 4: MFA / TOTP
<!-- execution: parallel -->
<!-- depends: phase2 -->

- [ ] Task 1: TOTP engine (`stitchd-core`)
  <!-- files: crates/stitchd-core/src/auth/totp.rs -->
  - [ ] Sub-task: Write failing tests (TOTP verify ok, ±1 window, wrong code,
        recovery code generation, single-use enforcement)
  - [ ] Sub-task: Update tech-stack.md — add totp-rs crate
  - [ ] Sub-task: `TotpEngine::generate_secret() -> (secret_bytes: Vec<u8>, qr_uri: String)`
  - [ ] Sub-task: `TotpEngine::verify(secret, code, window=1) -> bool`
  - [ ] Sub-task: `generate_recovery_codes(n=10) -> Vec<String>` (8-char alphanumeric)
  - [ ] Sub-task: Pass all tests

- [ ] Task 2: MFA repository (`stitchd-db`)
  <!-- files: crates/stitchd-db/src/auth/mfa.rs -->
  - [ ] Sub-task: Write failing integration tests
  - [ ] Sub-task: `create_challenge(user_id, ttl_secs) -> (MfaChallenge, raw_token)`
  - [ ] Sub-task: `consume_challenge(token_hash) -> Option<MfaChallenge>`
  - [ ] Sub-task: `enable_totp(user_id, encrypted_secret, recovery_code_hashes)`
  - [ ] Sub-task: `disable_totp(user_id)`
  - [ ] Sub-task: `get_totp_secret(user_id) -> Option<Vec<u8>>`
  - [ ] Sub-task: `consume_recovery_code(user_id, code_hash) -> bool`
  - [ ] Sub-task: Pass all tests

- [ ] Task 3: MFA handlers (`stitchd-server`)
  <!-- files: crates/stitchd-server/src/api/auth/mfa.rs -->
  <!-- depends: task1, task2 -->
  - [ ] Sub-task: Write failing handler tests (setup, confirm, challenge verify,
        recovery code, disable, policy enforcement)
  - [ ] Sub-task: POST /v1/users/me/mfa/setup → generate + encrypt secret → return QR URI
  - [ ] Sub-task: POST /v1/users/me/mfa/confirm {totp_code} → verify → enable →
        return single-display recovery codes
  - [ ] Sub-task: POST /auth/mfa/verify {challenge_token, totp_code|recovery_code}
        → validate → issue tokens
  - [ ] Sub-task: POST /v1/users/me/mfa/disable {totp_code}
  - [ ] Sub-task: POST /v1/users/me/mfa/recovery-codes/regenerate {totp_code}
  - [ ] Sub-task: OrgAdmin MFA policy (require_mfa on org config); enforce at login
        — redirect to setup if required and not enrolled
  - [ ] Sub-task: Pass all tests

- [ ] Task: Conductor - User Manual Verification 'Phase 4' (Protocol in workflow.md)

## Phase 5: OAuth2 / OIDC
<!-- execution: parallel -->
<!-- depends: phase2 -->

- [ ] Task 1: Auth provider repository (`stitchd-db`)
  <!-- files: crates/stitchd-db/src/auth/providers.rs -->
  - [ ] Sub-task: Write failing integration tests
  - [ ] Sub-task: `create(org_id, provider_type, display_name, config_encrypted)`
  - [ ] Sub-task: `find_by_id(id) -> Option<AuthProvider>`
  - [ ] Sub-task: `list_for_org(org_id) -> Vec<AuthProvider>`
  - [ ] Sub-task: `update(id, display_name, config_encrypted, enabled)`
  - [ ] Sub-task: `delete(id)` — enforce at-least-one-enabled constraint
  - [ ] Sub-task: Pass all tests

- [ ] Task 2: OIDC engine (`stitchd-core`)
  <!-- files: crates/stitchd-core/src/auth/oidc.rs -->
  - [ ] Sub-task: Write failing tests (discovery doc parse, auth URL generation,
        token exchange mock, userinfo extraction, Google + GitHub built-ins)
  - [ ] Sub-task: Update tech-stack.md — add openidconnect crate
  - [ ] Sub-task: `OidcProvider::from_discovery(url) -> Result<OidcProvider>`
  - [ ] Sub-task: `OidcProvider::authorization_url(state, nonce, redirect_uri) -> Url`
        with PKCE challenge
  - [ ] Sub-task: `OidcProvider::exchange_code(code, verifier, redirect_uri) -> IdToken`
  - [ ] Sub-task: `OidcProvider::email_from_id_token(token) -> Option<String>`
  - [ ] Sub-task: Built-in configs: `OidcProvider::google(client_id, secret)`,
        `OidcProvider::github(client_id, secret)`
  - [ ] Sub-task: Pass all tests

- [ ] Task 3: OIDC handlers + provider management API (`stitchd-server`)
  <!-- files: crates/stitchd-server/src/api/auth/oidc.rs,
              crates/stitchd-server/src/api/auth/providers.rs -->
  <!-- depends: task1, task2 -->
  - [ ] Sub-task: Write failing handler tests (authorize redirect, callback new user,
        callback existing user cross-org join, invalid state CSRF check)
  - [ ] Sub-task: Provider management CRUD:
        GET/POST/PUT/DELETE /v1/orgs/{org_id}/auth-providers
  - [ ] Sub-task: GET /auth/oidc/{org_slug}/{provider_id}/authorize →
        build URL + store PKCE+state in short-lived cache → redirect
  - [ ] Sub-task: GET /auth/oidc/{org_slug}/{provider_id}/callback →
        verify state → exchange code → extract email →
        find or create platform user → ensure org membership → issue tokens
  - [ ] Sub-task: Pass all tests

- [ ] Task: Conductor - User Manual Verification 'Phase 5' (Protocol in workflow.md)

## Phase 6: SAML 2.0
<!-- execution: parallel -->
<!-- depends: phase2 -->

- [ ] Task 1: SAML engine (`stitchd-core`)
  <!-- files: crates/stitchd-core/src/auth/saml.rs -->
  - [ ] Sub-task: Write failing tests (AuthnRequest generation, Response validation,
        attribute extraction, metadata XML, SLO request validation)
  - [ ] Sub-task: Update tech-stack.md — add samael crate
  - [ ] Sub-task: `SamlProvider::authn_request(acs_url, relay_state) -> (xml, redirect_url)`
  - [ ] Sub-task: `SamlProvider::validate_response(base64_response) -> Result<SamlEmail>`
        (validates IdP cert signature)
  - [ ] Sub-task: `SamlProvider::sp_metadata_xml(acs_url, entity_id) -> String`
  - [ ] Sub-task: `SamlProvider::validate_logout_request(xml) -> Result<SloContext>`
  - [ ] Sub-task: `SamlProvider::logout_response(in_response_to, relay_state) -> String`
  - [ ] Sub-task: Pass all tests

- [ ] Task 2: SAML handlers (`stitchd-server`)
  <!-- files: crates/stitchd-server/src/api/auth/saml.rs -->
  <!-- depends: task1 -->
  - [ ] Sub-task: Write failing handler tests (SP login redirect, ACS success,
        ACS invalid signature, SP metadata, SLO revokes sessions)
  - [ ] Sub-task: GET /auth/saml/{org_slug}/login → build AuthnRequest → redirect
  - [ ] Sub-task: POST /auth/saml/{org_slug}/acs → validate → extract email →
        find or create user → ensure org membership → issue tokens
  - [ ] Sub-task: GET /auth/saml/{org_slug}/metadata → SP metadata XML
  - [ ] Sub-task: POST /auth/saml/{org_slug}/slo → validate LogoutRequest →
        revoke org-scoped refresh tokens for user → return LogoutResponse XML
  - [ ] Sub-task: Pass all tests

- [ ] Task: Conductor - User Manual Verification 'Phase 6' (Protocol in workflow.md)

## Phase 7: User Lifecycle, Profile & Email Delivery
<!-- execution: parallel -->
<!-- depends: phase3 -->

- [ ] Task 1: Email delivery service (`stitchd-server`)
  <!-- files: crates/stitchd-server/src/email.rs -->
  - [ ] Sub-task: Write failing tests (SMTP send mock, offline fallback branch,
        EMAIL_REQUIRED=true rejection)
  - [ ] Sub-task: Update tech-stack.md — add lettre crate
  - [ ] Sub-task: `EmailService::send(to, subject, body) -> Result<Option<OfflineLink>>`
        using lettre + SMTP env vars (SMTP_HOST, SMTP_PORT, SMTP_USER,
        SMTP_PASSWORD, SMTP_FROM)
  - [ ] Sub-task: If SMTP unconfigured → return `Ok(Some(OfflineLink(url)))` instead of sending
  - [ ] Sub-task: If `EMAIL_REQUIRED=true` → return `Err` when SMTP unavailable
  - [ ] Sub-task: API handler helper: if `OfflineLink` present → inject
        `offline_link` field into JSON response
  - [ ] Sub-task: Pass all tests

- [ ] Task 2: Invite flow (`stitchd-db` + `stitchd-server`)
  <!-- files: crates/stitchd-db/src/auth/invites.rs,
              crates/stitchd-server/src/api/auth/invites.rs -->
  <!-- depends: task1 -->
  - [ ] Sub-task: Write failing tests (create, accept new user, accept existing
        user cross-org, expired token, already accepted)
  - [ ] Sub-task: `InviteRepository`: create, find_by_token_hash, accept,
        list_for_org, revoke
  - [ ] Sub-task: POST /v1/orgs/{org_id}/invites → create invite → email or offline_link
  - [ ] Sub-task: GET /v1/orgs/{org_id}/invites → list pending (OrgAdmin)
  - [ ] Sub-task: DELETE /v1/orgs/{org_id}/invites/{id} → revoke (OrgAdmin)
  - [ ] Sub-task: POST /auth/invites/{token}/accept {display_name, password?} →
        if new user: create + add membership;
        if existing user: add org_membership only (cross-org join)
  - [ ] Sub-task: Pass all tests

- [ ] Task 3: Password reset (`stitchd-db` + `stitchd-server`)
  <!-- files: crates/stitchd-db/src/auth/password_reset.rs,
              crates/stitchd-server/src/api/auth/password_reset.rs -->
  <!-- depends: task1 -->
  - [ ] Sub-task: Write failing tests (request, unknown email silent ok, OTP verify,
        expired OTP, already-used OTP)
  - [ ] Sub-task: OTP repository: create, find_valid_by_email, consume
  - [ ] Sub-task: POST /auth/password/reset-request {email} → silent on unknown;
        generate OTP → email or offline_link
  - [ ] Sub-task: POST /auth/password/reset {email, otp, new_password} →
        verify OTP → update hash → invalidate all sessions
  - [ ] Sub-task: Pass all tests

- [ ] Task 4: User profile & management (`stitchd-server`)
  <!-- files: crates/stitchd-server/src/api/auth/profile.rs,
              crates/stitchd-server/src/api/auth/user_management.rs -->
  - [ ] Sub-task: Write failing handler tests (me read/update, password change,
        OrgAdmin list/update/delete, role assignment guards)
  - [ ] Sub-task: GET /v1/users/me → profile + org memberships + MFA status
  - [ ] Sub-task: PUT /v1/users/me {display_name, avatar_url}
  - [ ] Sub-task: PUT /v1/users/me/password {current_password, new_password}
  - [ ] Sub-task: GET/PUT/DELETE /v1/orgs/{org_id}/users (RequireOrgRole(OrgAdmin))
  - [ ] Sub-task: PUT /v1/orgs/{org_id}/users/{id}/role
  - [ ] Sub-task: PUT /v1/projects/{project_id}/members/{user_id}/role
        (RequireProjectRole(ProjectAdmin))
  - [ ] Sub-task: PUT /v1/environments/{env_id}/members/{user_id}/role
        (RequireEnvRole(EnvPublisher))
  - [ ] Sub-task: Pass all tests

- [ ] Task: Conductor - User Manual Verification 'Phase 7' (Protocol in workflow.md)

## Phase 8: RBAC Hardening & Existing Endpoint Protection
<!-- depends: phase4, phase5, phase6, phase7 -->

- [ ] Task 1: Apply auth middleware to all existing routes
  <!-- files: crates/stitchd-server/src/api/router.rs -->
  - [ ] Sub-task: Write failing tests confirming each existing route returns 401
        without a valid JWT token
  - [ ] Sub-task: Apply `AuthLayer` to flags, segments, events, experiments,
        stats route trees
  - [ ] Sub-task: Mutation endpoints (create/update/delete) → RequireOrgRole(OrgAdmin)
  - [ ] Sub-task: Flag enable/disable in env → RequireEnvRole(EnvPublisher)
  - [ ] Sub-task: Pass all tests

- [ ] Task 2: SDK key / JWT segregation verification
  <!-- files: crates/stitchd-server/src/api/router.rs,
              crates/stitchd-server/src/api/sdk_auth.rs -->
  <!-- depends: task1 -->
  - [ ] Sub-task: Write failing tests: SDK endpoints reject JWT; admin endpoints
        reject SDK key
  - [ ] Sub-task: Verify SDK auth and JWT auth middleware are on disjoint router trees
  - [ ] Sub-task: Pass all tests

- [ ] Task 3: Rate limiting on auth endpoints
  <!-- files: crates/stitchd-server/src/api/auth/rate_limit.rs -->
  - [ ] Sub-task: Write failing tests (login rate limit triggers, reset rate limit)
  - [ ] Sub-task: Update tech-stack.md — add governor crate (tower-compatible leaky bucket)
  - [ ] Sub-task: Rate limiter on /auth/login, /auth/password/reset-request,
        /auth/mfa/verify
  - [ ] Sub-task: Pass all tests

- [ ] Task: Conductor - User Manual Verification 'Phase 8' (Protocol in workflow.md)
