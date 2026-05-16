-- Context type registry: tracks distinct (env_id, context_type) pairs seen
-- in evaluations within the last 90 days.
CREATE TABLE IF NOT EXISTS context_type_registry (
    env_id        UUID        NOT NULL,
    context_type  TEXT        NOT NULL,
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (env_id, context_type)
);

-- Context parameter registry: tracks distinct (env_id, context_type, param_key)
-- triples with their inferred type and privacy flag.
CREATE TABLE IF NOT EXISTS context_param_registry (
    env_id        UUID        NOT NULL,
    context_type  TEXT        NOT NULL,
    param_key     TEXT        NOT NULL,
    inferred_type TEXT        NOT NULL DEFAULT 'unknown'
                              CHECK (inferred_type IN ('str','int','double','bool','semver','unknown')),
    is_private    BOOLEAN     NOT NULL DEFAULT FALSE,
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (env_id, context_type, param_key)
);

-- Index for efficient 90-day lookups by env_id.
CREATE INDEX IF NOT EXISTS idx_context_type_registry_env_last
    ON context_type_registry (env_id, last_seen_at DESC);

CREATE INDEX IF NOT EXISTS idx_context_param_registry_env_type_last
    ON context_param_registry (env_id, context_type, last_seen_at DESC);
