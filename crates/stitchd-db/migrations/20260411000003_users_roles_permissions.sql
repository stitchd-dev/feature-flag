-- Migration 003: users, roles, permissions, user_project_roles
--
-- RBAC layer. Users exist at organisation scope; roles are project-scoped.
-- user_project_roles assigns a role to a user within a specific project.

-- ---------------------------------------------------------------------------
-- users
-- ---------------------------------------------------------------------------
CREATE TABLE users (
    id               UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    organisation_id  UUID        NOT NULL REFERENCES organisations(id),
    email            TEXT        NOT NULL,
    password_hash    TEXT        NOT NULL,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at       TIMESTAMPTZ,
    version          BIGINT      NOT NULL DEFAULT 1,
    UNIQUE (email, organisation_id)
);

CREATE INDEX idx_users_organisation_id ON users(organisation_id);

-- ---------------------------------------------------------------------------
-- roles
-- ---------------------------------------------------------------------------
CREATE TABLE roles (
    id          UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    project_id  UUID        NOT NULL REFERENCES projects(id),
    name        TEXT        NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at  TIMESTAMPTZ,
    version     BIGINT      NOT NULL DEFAULT 1
);

CREATE INDEX idx_roles_project_id ON roles(project_id);

-- ---------------------------------------------------------------------------
-- permissions
-- ---------------------------------------------------------------------------
CREATE TABLE permissions (
    id                UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    role_id           UUID        NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    resource_type     TEXT        NOT NULL,
    resource_pattern  TEXT        NOT NULL,
    action            TEXT        NOT NULL
);

CREATE INDEX idx_permissions_role_id ON permissions(role_id);

-- ---------------------------------------------------------------------------
-- user_project_roles
-- ---------------------------------------------------------------------------
CREATE TABLE user_project_roles (
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    project_id  UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    role_id     UUID NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    UNIQUE (user_id, project_id, role_id)
);

CREATE INDEX idx_user_project_roles_user_id    ON user_project_roles(user_id);
CREATE INDEX idx_user_project_roles_project_id ON user_project_roles(project_id);
