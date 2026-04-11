-- Migration 005: segments
--
-- Segments are environment-scoped. Two types exist: rule-based and list-based.
-- Rule content and list members are stored in separate tables (added in later tracks).

CREATE TABLE segments (
    id              UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    environment_id  UUID        NOT NULL REFERENCES environments(id),
    key             TEXT        NOT NULL,
    segment_type    TEXT        NOT NULL CHECK (segment_type IN ('rule', 'list')),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at      TIMESTAMPTZ,
    version         BIGINT      NOT NULL DEFAULT 1,
    UNIQUE (key, environment_id)
);

CREATE INDEX idx_segments_environment_id ON segments(environment_id);
