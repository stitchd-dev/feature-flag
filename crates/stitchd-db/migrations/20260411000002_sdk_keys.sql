-- Migration 002: sdk_keys
--
-- SDK authentication keys scoped to an environment.
-- Raw key is never stored — only the hash.
-- Composite index supports the "find all active keys for environment" query.

CREATE TABLE sdk_keys (
    id              UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    environment_id  UUID        NOT NULL REFERENCES environments(id),
    key_hash        TEXT        NOT NULL,
    is_active       BOOLEAN     NOT NULL DEFAULT TRUE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at      TIMESTAMPTZ
);

CREATE INDEX idx_sdk_keys_environment_active ON sdk_keys(environment_id, is_active);
