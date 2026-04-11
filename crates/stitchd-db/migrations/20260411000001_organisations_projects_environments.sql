-- Migration 001: organisations, projects, environments
--
-- Establishes the top-level tenancy hierarchy.
-- All three tables share the same soft-delete + optimistic-concurrency pattern.

CREATE EXTENSION IF NOT EXISTS "pgcrypto";  -- for gen_random_uuid()

-- ---------------------------------------------------------------------------
-- organisations
-- ---------------------------------------------------------------------------
CREATE TABLE organisations (
    id          UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    name        TEXT        NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at  TIMESTAMPTZ,
    version     BIGINT      NOT NULL DEFAULT 1
);

-- ---------------------------------------------------------------------------
-- projects
-- ---------------------------------------------------------------------------
CREATE TABLE projects (
    id               UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    organisation_id  UUID        NOT NULL REFERENCES organisations(id),
    name             TEXT        NOT NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at       TIMESTAMPTZ,
    version          BIGINT      NOT NULL DEFAULT 1
);

CREATE INDEX idx_projects_organisation_id ON projects(organisation_id);

-- ---------------------------------------------------------------------------
-- environments
-- ---------------------------------------------------------------------------
CREATE TABLE environments (
    id          UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    project_id  UUID        NOT NULL REFERENCES projects(id),
    name        TEXT        NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at  TIMESTAMPTZ,
    version     BIGINT      NOT NULL DEFAULT 1
);

CREATE INDEX idx_environments_project_id ON environments(project_id);
