-- Migration 20260419000001: event_definitions
--
-- Pre-registered event definitions scoped per environment.
-- Only keys registered here are accepted at the ingestion boundary.
-- value_type constrains which numeric field may be populated when an event is ingested.

CREATE TABLE event_definitions (
    id              UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    environment_id  UUID        NOT NULL REFERENCES environments(id),
    key             TEXT        NOT NULL,
    value_type      TEXT        NOT NULL CHECK (value_type IN ('bool', 'int', 'double')),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at      TIMESTAMPTZ,
    version         BIGINT      NOT NULL DEFAULT 1,
    UNIQUE (key, environment_id)
);

CREATE INDEX idx_event_definitions_environment_id ON event_definitions(environment_id);
