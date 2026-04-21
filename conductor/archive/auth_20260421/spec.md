# Spec: JWT / Multi-Mechanism Human Auth

## Overview

Implement the complete human authentication layer for the Admin API.

**Identity model:** Users are platform-level entities identified globally by
`email_id`. Org membership is a separate join — one user account can belong to
multiple orgs without duplicate records. A user logs in once and holds an
org-scoped session context in their token.

**Token model:** Short-lived JWT access tokens signed with a per-user
`token_secret`; long-lived opaque refresh tokens each stored as a DB row.
"Sign out all devices" rotates `token_secret`. Active sessions are governed by
non-revoked, non-expired refresh token entries.

**Auth mechanisms (per-org, multi-select):** password, OAuth2/OIDC, SAML 2.0.
OrgAdmin configures and enables any combination.

**Email delivery:** optional — SMTP configured via env vars. If unconfigured,
invite/reset/MFA-recovery links are returned directly in the API response
(offline delivery) so SuperAdmin/OrgAdmin can share them out-of-band.

## Functional Requirements

### 1. Identity & User Model (Platform-Level)
- `users` table (platform-level, not per-org):
  id, email (globally unique), display_name, avatar_url, password_hash
  (nullable), token_secret (UUID), totp_secret (nullable, encrypted),
  totp_enabled, status (active/deactivated), created_at, updated_at
- `org_memberships` table: user_id, org_id, role (OrgAdmin|OrgMember),
  joined_at — one user can be a member of N orgs
- `user_project_roles` table: user_id, project_id, role
- `user_env_roles` table: user_id, env_id, role
- A user who is a member of multiple orgs selects active org context at login
  or via a token-swap endpoint; access token carries org_id claim

### 2. Token Design
- **Access token**: short-lived JWT (15 min), HMAC-SHA256 signed with per-user
  `token_secret`; claims: user_id, org_id, email, org_role, exp, iat
- **Refresh token**: opaque random bytes, stored hashed in `refresh_tokens`
  (id, user_id, org_id, token_hash, device_hint, issued_at, expires_at,
  revoked_at, last_used_at); default TTL 30 days sliding
- **Sign out all devices**: atomically rotate `token_secret` + bulk-revoke all
  refresh tokens for user
- **Active sessions**: refresh_tokens WHERE revoked_at IS NULL AND
  expires_at > now()
- **Org context switch**: POST /auth/switch-org {org_id} → validates membership
  → issues new access + refresh token pair for that org

### 3. Auth Mechanisms

#### 3a. Username / Password
- Registration via invite-only (see §6)
- Argon2id password hashing
- POST /auth/login → returns access_token + refresh_token
  (if MFA enabled: returns `mfa_challenge_token` instead; client must complete
  TOTP step before receiving real tokens)
- Password reset: email OTP (6-digit, 10 min TTL) or offline link (see §9)

#### 3b. OAuth2 / OIDC
- Per-org OIDC provider config: client_id, client_secret (AES-256-GCM
  encrypted), discovery_url or manual endpoints, provider_type hint
  (google | github | custom)
- Built-in types pre-fill discovery URLs for Google and GitHub
- Callback: GET /auth/oidc/{org_slug}/{provider_id}/callback →
  exchange code → userinfo → match user by email → issue tokens
  (creates platform user on first login if invite exists for that email)
- Per-org redirect URI: /auth/oidc/{org_slug}/{provider_id}/callback
- PKCE + state param for CSRF protection

#### 3c. SAML 2.0
- Per-org SAML IdP config: entity_id, SSO URL, IdP signing cert (PEM),
  attribute mapping (email, display_name)
- SP-initiated SSO: GET /auth/saml/{org_slug}/login
- ACS: POST /auth/saml/{org_slug}/acs → validate assertion → match by email
  → issue tokens
- SP metadata: GET /auth/saml/{org_slug}/metadata → XML
- **IdP-initiated logout (SLO):**
  POST /auth/saml/{org_slug}/slo → validate LogoutRequest →
  revoke all org-scoped refresh tokens for user → return LogoutResponse XML

### 4. MFA — TOTP (Authenticator App)
- TOTP per RFC 6238 (Google Authenticator, Authy, 1Password compatible)
- Setup flow:
  1. POST /v1/users/me/mfa/setup → returns TOTP secret + QR code URI
  2. POST /v1/users/me/mfa/confirm {totp_code} → verifies code → activates MFA
     → returns single-display recovery codes (10 × 8-char alphanumeric,
       stored as Argon2id hashes)
- Login with MFA:
  1. POST /auth/login → `{mfa_required: true, challenge_token: "<short-lived>"}`
  2. POST /auth/mfa/verify {challenge_token, totp_code | recovery_code}
     → issues access_token + refresh_token
- Disable MFA: POST /v1/users/me/mfa/disable {totp_code} (requires current TOTP)
- OrgAdmin MFA policy: org can require MFA for all members
- Recovery codes: single-use; POST /v1/users/me/mfa/recovery-codes/regenerate
  (requires current TOTP)

### 5. Provider Management (per org)
- `auth_providers` table: id, org_id, provider_type (password|oidc|saml),
  display_name, config JSONB (AES-256-GCM encrypted), enabled, created_at
- API: CRUD /v1/orgs/{org_id}/auth-providers
- At least one enabled provider enforced; OrgAdmin enables/disables providers

### 6. User Lifecycle

#### Invite Flow
- OrgAdmin: POST /v1/orgs/{org_id}/invites {email, org_role}
  → creates invite row + sends invite email OR returns offline link if SMTP
    not configured (see §9)
- `invites` table: id, org_id, email, org_role, invited_by_user_id,
  token_hash, expires_at (72h), accepted_at
- Accept: POST /auth/invites/{token}/accept
  → if user already exists (cross-org): adds org_membership, skips user creation
  → if new user: creates platform user record, then adds membership

#### Password Reset
- POST /auth/password/reset-request {email} → OTP email or offline link
- POST /auth/password/reset {email, otp, new_password}

#### User Management (OrgAdmin)
- GET    /v1/orgs/{org_id}/users — paginated, filterable
- GET    /v1/orgs/{org_id}/users/{id}
- PUT    /v1/orgs/{org_id}/users/{id} — update status, role
- DELETE /v1/orgs/{org_id}/users/{id} — removes org membership (does not
  delete platform user; user may still belong to other orgs)

#### User Profile (self-service)
- GET /v1/users/me — full profile (orgs, roles, MFA status)
- PUT /v1/users/me — update display_name, avatar_url
- PUT /v1/users/me/password — change password (requires current password)
- Avatar: stored as URL (external CDN) or base64-encoded blob in DB;
  configurable via env (AVATAR_STORAGE = url | db)

### 7. RBAC — 4-Level Hierarchy

| Level       | Roles                        |
|-------------|------------------------------|
| Org         | OrgAdmin, OrgMember          |
| Project     | ProjectAdmin, ProjectViewer  |
| Environment | EnvPublisher, EnvViewer      |

- Org role carried in access token (org_role claim)
- Project and env roles queried from DB and cached in request-scoped state
- Axum extractors: `RequireOrgRole(OrgAdmin)`, `RequireProjectRole(ProjectAdmin)`,
  `RequireEnvRole(EnvPublisher)` — return 403 on failure
- Auth middleware guards all existing admin API endpoints

### 8. Session Management
- GET    /auth/sessions — caller's active sessions
- DELETE /auth/sessions/{id} — revoke specific session
- DELETE /auth/sessions — sign out all devices
- POST   /auth/refresh {refresh_token} → new access + refresh pair;
  old refresh token consumed (rotation)
- POST   /auth/logout — revoke current session's refresh token
- POST   /auth/switch-org {org_id} — swap org context; new token pair

### 9. Email Delivery & Offline Fallback
- SMTP configured via env vars: SMTP_HOST, SMTP_PORT, SMTP_USER,
  SMTP_PASSWORD, SMTP_FROM (all optional)
- `lettre` crate for SMTP delivery
- **If SMTP not configured:**
  - Invite / reset / MFA-recovery-code regeneration API responses include
    `{"offline_link": "<full tokenized URL>"}` field in the JSON response
  - SuperAdmin / OrgAdmin can copy this URL and deliver it out-of-band
  - Applies to: invite creation, password reset request, recovery code regenerate
- Platform flag `EMAIL_REQUIRED=true` (default false) — if set, disables
  offline fallback and rejects the operation when SMTP is unavailable

## Non-Functional Requirements
- Argon2id for passwords and recovery code hashes (argon2 crate)
- AES-256-GCM for OIDC/SAML/TOTP secrets at rest (aes-gcm crate)
- TOTP: HMAC-SHA1, 6-digit, 30s window ± 1 step drift tolerance (totp-rs crate)
- Rate limiting on /auth/login, /auth/password/reset-request, /auth/mfa/verify
  (tower leaky-bucket middleware)
- OpenTelemetry spans on all auth paths
- utoipa OpenAPI annotations
- Coverage ≥ 90% on new code

## Acceptance Criteria
- [ ] Password login returns access + refresh tokens; 15 min access TTL enforced
- [ ] "Sign out all devices" rotates token_secret; old access tokens rejected
- [ ] Refresh token exchange issues new pair; old token consumed
- [ ] OIDC flow (Google + GitHub) resolves user by email; cross-org join works
- [ ] SAML SP-initiated login issues tokens; IdP-initiated SLO revokes sessions
- [ ] TOTP setup, login challenge, and recovery code flow complete end-to-end
- [ ] OrgAdmin MFA policy enforced on login
- [ ] Invite accepted by new user creates platform account + org membership
- [ ] Invite accepted by existing user adds org membership only (no duplicate)
- [ ] Cross-org switch issues org-scoped token pair
- [ ] User profile (GET/PUT /v1/users/me) works including avatar
- [ ] SMTP configured → emails sent; unconfigured → offline_link in response
- [ ] RBAC extractors return 403 on insufficient role
- [ ] All existing admin endpoints protected by auth middleware
- [ ] Coverage ≥ 90%

## Out of Scope
- WebAuthn / passkeys
- SMS-based OTP
- Org creation (bootstrap via seed/CLI)
- Advanced audit log UI (auth events written to existing audit log table)
