-- Migration 004: feature_flags, variants
--
-- Feature flag definitions are project-scoped.
-- Variants belong to a flag and carry typed values as JSONB.

-- ---------------------------------------------------------------------------
-- feature_flags
-- ---------------------------------------------------------------------------
CREATE TABLE feature_flags (
    id          UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    project_id  UUID        NOT NULL REFERENCES projects(id),
    key         TEXT        NOT NULL,
    value_type  TEXT        NOT NULL,
    enabled     BOOLEAN     NOT NULL DEFAULT FALSE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at  TIMESTAMPTZ,
    version     BIGINT      NOT NULL DEFAULT 1,
    UNIQUE (key, project_id)
);

CREATE INDEX idx_feature_flags_project_id ON feature_flags(project_id);

-- ---------------------------------------------------------------------------
-- variants
-- ---------------------------------------------------------------------------
CREATE TABLE variants (
    id       UUID    NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    flag_id  UUID    NOT NULL REFERENCES feature_flags(id) ON DELETE CASCADE,
    key      TEXT    NOT NULL,
    value    JSONB   NOT NULL,
    UNIQUE (key, flag_id)
);

CREATE INDEX idx_variants_flag_id ON variants(flag_id);
