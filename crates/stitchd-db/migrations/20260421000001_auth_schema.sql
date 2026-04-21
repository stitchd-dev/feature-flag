-- Migration 20260421000001: auth schema
--
-- Introduces a platform-level auth system:
--   - Replaces the per-org users table with a platform-level users table
--   - Replaces the old user_project_roles (role_id FK) with a direct TEXT role column
--   - Adds org_memberships, user_env_roles, refresh_tokens, auth_providers,
--     invites, mfa_challenges, mfa_recovery_codes, password_reset_otps

-- ---------------------------------------------------------------------------
-- Step 1: Drop old tables that are being replaced
-- ---------------------------------------------------------------------------

-- Drop old user_project_roles first (FK onto users and projects)
DROP TABLE IF EXISTS user_project_roles;

-- Drop old users table (was per-org scoped)
DROP TABLE IF EXISTS users;

-- ---------------------------------------------------------------------------
-- Step 2: platform-level users
-- ---------------------------------------------------------------------------
CREATE TABLE users (
    id             UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    email          TEXT        NOT NULL,
    display_name   TEXT        NOT NULL,
    avatar_url     TEXT,
    password_hash  TEXT,
    token_secret   UUID        NOT NULL DEFAULT gen_random_uuid(),
    totp_secret    BYTEA,
    totp_enabled   BOOLEAN     NOT NULL DEFAULT false,
    status         TEXT        NOT NULL DEFAULT 'active'
                       CONSTRAINT chk_users_status CHECK (status IN ('active', 'deactivated')),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT uq_users_email UNIQUE (email)
);

-- ---------------------------------------------------------------------------
-- Step 3: org_memberships
-- ---------------------------------------------------------------------------
CREATE TABLE org_memberships (
    user_id    UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    org_id     UUID        NOT NULL REFERENCES organisations(id) ON DELETE CASCADE,
    role       TEXT        NOT NULL
                   CONSTRAINT chk_org_memberships_role CHECK (role IN ('org_admin', 'org_member')),
    joined_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT pk_org_memberships PRIMARY KEY (user_id, org_id)
);

-- ---------------------------------------------------------------------------
-- Step 4: user_project_roles (new schema — direct TEXT role, no role_id FK)
-- ---------------------------------------------------------------------------
CREATE TABLE user_project_roles (
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    project_id  UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    role        TEXT NOT NULL
                    CONSTRAINT chk_user_project_roles_role CHECK (role IN ('project_admin', 'project_viewer')),
    CONSTRAINT pk_user_project_roles PRIMARY KEY (user_id, project_id)
);

-- ---------------------------------------------------------------------------
-- Step 5: user_env_roles
-- ---------------------------------------------------------------------------
CREATE TABLE user_env_roles (
    user_id  UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    env_id   UUID NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
    role     TEXT NOT NULL
                 CONSTRAINT chk_user_env_roles_role CHECK (role IN ('env_publisher', 'env_viewer')),
    CONSTRAINT pk_user_env_roles PRIMARY KEY (user_id, env_id)
);

-- ---------------------------------------------------------------------------
-- Step 6: refresh_tokens
-- ---------------------------------------------------------------------------
CREATE TABLE refresh_tokens (
    id            UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    user_id       UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    org_id        UUID        NOT NULL REFERENCES organisations(id) ON DELETE CASCADE,
    token_hash    TEXT        NOT NULL,
    device_hint   TEXT,
    issued_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at    TIMESTAMPTZ NOT NULL,
    revoked_at    TIMESTAMPTZ,
    last_used_at  TIMESTAMPTZ,
    CONSTRAINT uq_refresh_tokens_token_hash UNIQUE (token_hash)
);

CREATE INDEX idx_refresh_tokens_user_revoked_expires
    ON refresh_tokens (user_id, revoked_at, expires_at);

-- ---------------------------------------------------------------------------
-- Step 7: auth_providers
-- ---------------------------------------------------------------------------
CREATE TABLE auth_providers (
    id             UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    org_id         UUID        NOT NULL REFERENCES organisations(id) ON DELETE CASCADE,
    provider_type  TEXT        NOT NULL
                       CONSTRAINT chk_auth_providers_type CHECK (provider_type IN ('password', 'oidc', 'saml')),
    display_name   TEXT        NOT NULL,
    config         JSONB       NOT NULL DEFAULT '{}',
    enabled        BOOLEAN     NOT NULL DEFAULT true,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ---------------------------------------------------------------------------
-- Step 8: invites
-- ---------------------------------------------------------------------------
CREATE TABLE invites (
    id                 UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    org_id             UUID        NOT NULL REFERENCES organisations(id) ON DELETE CASCADE,
    email              TEXT        NOT NULL,
    org_role           TEXT        NOT NULL
                           CONSTRAINT chk_invites_org_role CHECK (org_role IN ('org_admin', 'org_member')),
    invited_by_user_id UUID        REFERENCES users(id) ON DELETE SET NULL,
    token_hash         TEXT        NOT NULL,
    expires_at         TIMESTAMPTZ NOT NULL,
    accepted_at        TIMESTAMPTZ,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT uq_invites_token_hash UNIQUE (token_hash)
);

-- ---------------------------------------------------------------------------
-- Step 9: mfa_challenges
-- ---------------------------------------------------------------------------
CREATE TABLE mfa_challenges (
    id                    UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    user_id               UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    challenge_token_hash  TEXT        NOT NULL,
    expires_at            TIMESTAMPTZ NOT NULL,
    used_at               TIMESTAMPTZ,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT uq_mfa_challenges_token_hash UNIQUE (challenge_token_hash)
);

-- ---------------------------------------------------------------------------
-- Step 10: mfa_recovery_codes
-- ---------------------------------------------------------------------------
CREATE TABLE mfa_recovery_codes (
    id          UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    user_id     UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_hash   TEXT        NOT NULL,
    used_at     TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ---------------------------------------------------------------------------
-- Step 11: password_reset_otps
-- ---------------------------------------------------------------------------
CREATE TABLE password_reset_otps (
    id          UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    email       TEXT        NOT NULL,
    otp_hash    TEXT        NOT NULL,
    expires_at  TIMESTAMPTZ NOT NULL,
    used_at     TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
