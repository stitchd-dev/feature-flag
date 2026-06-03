-- =============================================================================
-- V1 Baseline — stitchd-db PostgreSQL schema (post-cutover 2026-05-25)
-- Synthesised from migrations 20260411000001 through 20260524000001.
--
-- Retired tables (intentionally omitted):
--   segment_rules        — retired in this cutover
--   segment_list_entries — dropped in 20260516000005 (moved to ScyllaDB)
--   experiment_results   — dropped in 20260519000001 (moved to ClickHouse)
--   flag_evaluation_log  — lives in ClickHouse, never in Postgres
-- =============================================================================

-- ---------------------------------------------------------------------------
-- Extensions
-- ---------------------------------------------------------------------------
CREATE EXTENSION IF NOT EXISTS "pgcrypto";

-- ---------------------------------------------------------------------------
-- organisations
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS organisations (
    id         UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    name       TEXT        NOT NULL,
    is_system  BOOLEAN     NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    version    BIGINT      NOT NULL DEFAULT 1
);

-- ---------------------------------------------------------------------------
-- projects
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS projects (
    id              UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    organisation_id UUID        NOT NULL REFERENCES organisations(id),
    name            TEXT        NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at      TIMESTAMPTZ,
    version         BIGINT      NOT NULL DEFAULT 1
);

-- ---------------------------------------------------------------------------
-- environments
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS environments (
    id         UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    project_id UUID        NOT NULL REFERENCES projects(id),
    name       TEXT        NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    version    BIGINT      NOT NULL DEFAULT 1
);

-- ---------------------------------------------------------------------------
-- users
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS users (
    id            UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    email         TEXT        NOT NULL,
    display_name  TEXT        NOT NULL,
    avatar_url    TEXT,
    password_hash TEXT,
    token_secret  UUID        NOT NULL DEFAULT gen_random_uuid(),
    totp_secret   BYTEA,
    totp_enabled  BOOLEAN     NOT NULL DEFAULT false,
    status        TEXT        NOT NULL DEFAULT 'active'
                      CONSTRAINT chk_users_status CHECK (status IN ('active', 'deactivated')),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT uq_users_email UNIQUE (email)
);

-- ---------------------------------------------------------------------------
-- org_memberships
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS org_memberships (
    user_id   UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    org_id    UUID        NOT NULL REFERENCES organisations(id) ON DELETE CASCADE,
    role      TEXT        NOT NULL
                  CONSTRAINT chk_org_memberships_role CHECK (role IN ('org_admin', 'org_member')),
    joined_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT pk_org_memberships PRIMARY KEY (user_id, org_id)
);

-- ---------------------------------------------------------------------------
-- user_project_roles
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS user_project_roles (
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    role       TEXT NOT NULL
                   CONSTRAINT chk_user_project_roles_role CHECK (role IN ('project_admin', 'project_viewer')),
    CONSTRAINT pk_user_project_roles PRIMARY KEY (user_id, project_id)
);

-- ---------------------------------------------------------------------------
-- user_env_roles
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS user_env_roles (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    env_id  UUID NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
    role    TEXT NOT NULL
                CONSTRAINT chk_user_env_roles_role CHECK (role IN ('env_publisher', 'env_viewer')),
    CONSTRAINT pk_user_env_roles PRIMARY KEY (user_id, env_id)
);

-- ---------------------------------------------------------------------------
-- refresh_tokens
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS refresh_tokens (
    id           UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    user_id      UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    org_id       UUID        NOT NULL REFERENCES organisations(id) ON DELETE CASCADE,
    token_hash   TEXT        NOT NULL,
    device_hint  TEXT,
    issued_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at   TIMESTAMPTZ NOT NULL,
    revoked_at   TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ,
    CONSTRAINT uq_refresh_tokens_token_hash UNIQUE (token_hash)
);

-- ---------------------------------------------------------------------------
-- auth_providers
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS auth_providers (
    id            UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    org_id        UUID        NOT NULL REFERENCES organisations(id) ON DELETE CASCADE,
    provider_type TEXT        NOT NULL
                      CONSTRAINT chk_auth_providers_type CHECK (provider_type IN ('password', 'oidc', 'saml')),
    display_name  TEXT        NOT NULL,
    config        JSONB       NOT NULL DEFAULT '{}',
    enabled       BOOLEAN     NOT NULL DEFAULT true,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ---------------------------------------------------------------------------
-- invites
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS invites (
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
-- mfa_challenges
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS mfa_challenges (
    id                   UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    user_id              UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    challenge_token_hash TEXT        NOT NULL,
    expires_at           TIMESTAMPTZ NOT NULL,
    used_at              TIMESTAMPTZ,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT uq_mfa_challenges_token_hash UNIQUE (challenge_token_hash)
);

-- ---------------------------------------------------------------------------
-- mfa_recovery_codes
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS mfa_recovery_codes (
    id         UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    user_id    UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_hash  TEXT        NOT NULL,
    used_at    TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ---------------------------------------------------------------------------
-- password_reset_otps
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS password_reset_otps (
    id         UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    email      TEXT        NOT NULL,
    otp_hash   TEXT        NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    used_at    TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ---------------------------------------------------------------------------
-- roles
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS roles (
    id         UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    project_id UUID        NOT NULL REFERENCES projects(id),
    name       TEXT        NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    version    BIGINT      NOT NULL DEFAULT 1
);

-- ---------------------------------------------------------------------------
-- permissions
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS permissions (
    id               UUID NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    role_id          UUID NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    resource_type    TEXT NOT NULL,
    resource_pattern TEXT NOT NULL,
    action           TEXT NOT NULL
);

-- ---------------------------------------------------------------------------
-- sdk_keys  (name column from 20260524000001)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS sdk_keys (
    id             UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    environment_id UUID        NOT NULL REFERENCES environments(id),
    key_hash       TEXT        NOT NULL,
    name           TEXT        NOT NULL DEFAULT '',
    is_active      BOOLEAN     NOT NULL DEFAULT TRUE,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at     TIMESTAMPTZ
);

-- ---------------------------------------------------------------------------
-- feature_flags  (WITHOUT default_variant_id — added after variants; circular FK)
-- Folds in: 20260512000001 (name, description), 20260521000002 (default_rule_distribution),
--           20260522000001 (default_rule_hash_inputs)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS feature_flags (
    id                        UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    project_id                UUID        NOT NULL REFERENCES projects(id),
    key                       TEXT        NOT NULL,
    name                      TEXT        NOT NULL DEFAULT '',
    description               TEXT        NOT NULL DEFAULT '',
    value_type                TEXT        NOT NULL,
    enabled                   BOOLEAN     NOT NULL DEFAULT FALSE,
    default_rule_distribution JSONB,
    default_rule_hash_inputs  JSONB,
    created_at                TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at                TIMESTAMPTZ,
    version                   BIGINT      NOT NULL DEFAULT 1,
    UNIQUE (key, project_id)
);

-- ---------------------------------------------------------------------------
-- variants
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS variants (
    id      UUID  NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    flag_id UUID  NOT NULL REFERENCES feature_flags(id) ON DELETE CASCADE,
    key     TEXT  NOT NULL,
    value   JSONB NOT NULL,
    UNIQUE (key, flag_id)
);

-- Circular FK: feature_flags.default_variant_id → variants(id)
ALTER TABLE feature_flags
    ADD COLUMN IF NOT EXISTS default_variant_id UUID REFERENCES variants(id);

-- ---------------------------------------------------------------------------
-- flag_hashing_config
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS flag_hashing_config (
    id             UUID NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    flag_id        UUID NOT NULL REFERENCES feature_flags(id) ON DELETE CASCADE,
    parameter_key  TEXT NOT NULL,
    parameter_type TEXT NOT NULL,
    "order"        INT  NOT NULL DEFAULT 0,
    UNIQUE (flag_id, parameter_key, parameter_type)
);

-- ---------------------------------------------------------------------------
-- feature_flag_rules  (frozen from 20260419000002; hash_inputs from 20260522000001)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS feature_flag_rules (
    id          UUID    NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    flag_id     UUID    NOT NULL REFERENCES feature_flags(id) ON DELETE CASCADE,
    rule_index  INT     NOT NULL,
    rule_def    JSONB   NOT NULL,
    frozen      BOOLEAN NOT NULL DEFAULT FALSE,
    hash_inputs JSONB   NULL,
    UNIQUE (flag_id, rule_index)
);

-- ---------------------------------------------------------------------------
-- segments  (name/description/tags from 20260513000001; condition_expr from 20260513000002)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS segments (
    id             UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    environment_id UUID        NOT NULL REFERENCES environments(id),
    key            TEXT        NOT NULL,
    name           TEXT        NOT NULL DEFAULT '',
    description    TEXT        NOT NULL DEFAULT '',
    tags           TEXT[]      NOT NULL DEFAULT '{}',
    segment_type   TEXT        NOT NULL CHECK (segment_type IN ('rule', 'list')),
    condition_expr JSONB,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at     TIMESTAMPTZ,
    version        BIGINT      NOT NULL DEFAULT 1,
    UNIQUE (key, environment_id)
);

-- ---------------------------------------------------------------------------
-- audit_log
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS audit_log (
    id            UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    actor_id      UUID,
    resource_type TEXT        NOT NULL,
    resource_id   UUID        NOT NULL,
    action        TEXT        NOT NULL,
    diff          JSONB       NOT NULL DEFAULT '{}',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ---------------------------------------------------------------------------
-- event_definitions  (admin fields from 20260520000004; name NOT NULL on fresh deploy)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS event_definitions (
    id             UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    environment_id UUID        NOT NULL REFERENCES environments(id),
    key            TEXT        NOT NULL,
    value_type     TEXT        NOT NULL CHECK (value_type IN ('bool', 'int', 'double')),
    name           TEXT        NOT NULL DEFAULT '',
    description    TEXT,
    metric_type    TEXT        NOT NULL DEFAULT 'count'
                               CHECK (metric_type IN ('count', 'conversion', 'revenue', 'duration', 'numeric', 'custom')),
    schema         JSONB,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at     TIMESTAMPTZ,
    version        BIGINT      NOT NULL DEFAULT 1,
    UNIQUE (key, environment_id)
);

-- ---------------------------------------------------------------------------
-- metric_definitions
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS metric_definitions (
    id             UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    environment_id UUID        NOT NULL REFERENCES environments(id),
    key            TEXT        NOT NULL,
    name           TEXT        NOT NULL,
    description    TEXT,
    kind           TEXT        NOT NULL CHECK (kind IN ('aggregation', 'ratio', 'funnel')),
    config         JSONB       NOT NULL,
    goal_direction TEXT        NOT NULL DEFAULT 'increase'
                               CHECK (goal_direction IN ('increase', 'decrease', 'neutral')),
    version        BIGINT      NOT NULL DEFAULT 1,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at     TIMESTAMPTZ
);

-- ---------------------------------------------------------------------------
-- experiments  (bigint from 20260419000003; metric_ids from 20260520000002;
--               attribution fields from 20260521000001)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS experiments (
    id                   UUID          NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    env_id               UUID          NOT NULL REFERENCES environments(id),
    flag_id              UUID          NOT NULL REFERENCES feature_flags(id),
    flag_rule_id         UUID          REFERENCES feature_flag_rules(id),
    name                 TEXT          NOT NULL,
    description          TEXT,
    hypothesis           TEXT,
    status               TEXT          NOT NULL
                             CHECK (status IN ('draft', 'running', 'paused', 'stopped')),
    metric_ids           UUID[]        NOT NULL DEFAULT '{}',
    guardrail_metric_ids UUID[]        NOT NULL DEFAULT '{}',
    traffic_allocation   NUMERIC(5,1)  NOT NULL DEFAULT 100.0,
    min_sample_size      BIGINT,
    targets_default_rule BOOLEAN       NOT NULL DEFAULT FALSE,
    pre_period_days      INT           NOT NULL DEFAULT 0,
    unit_context_types   TEXT[]        NOT NULL DEFAULT '{user}',
    scheduled_start_at   TIMESTAMPTZ,
    scheduled_end_at     TIMESTAMPTZ,
    -- Sequential testing (always-valid mSPRT / GAVI) configuration.
    sequential_testing_enabled BOOLEAN          NOT NULL DEFAULT FALSE,
    sequential_alpha           DOUBLE PRECISION NOT NULL DEFAULT 0.05,
    -- NULLABLE: null = auto-derive the mixing variance at compute time.
    sequential_tau_squared     DOUBLE PRECISION,
    sequential_min_sample_size BIGINT           NOT NULL DEFAULT 100,
    version              BIGINT        NOT NULL DEFAULT 1,
    created_at           TIMESTAMPTZ   NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ   NOT NULL DEFAULT now(),
    deleted_at           TIMESTAMPTZ,
    CONSTRAINT experiments_rule_xor_default CHECK (
        (flag_rule_id IS NOT NULL)::int + targets_default_rule::int = 1
    ),
    CONSTRAINT experiments_pre_period_days_nonneg CHECK (pre_period_days >= 0),
    CONSTRAINT experiments_unit_context_types_nonempty CHECK (
        cardinality(unit_context_types) > 0
    ),
    CONSTRAINT experiments_sequential_alpha_range CHECK (
        sequential_alpha > 0 AND sequential_alpha < 1
    ),
    CONSTRAINT experiments_sequential_tau_squared_positive CHECK (
        sequential_tau_squared IS NULL OR sequential_tau_squared > 0
    ),
    CONSTRAINT experiments_sequential_min_sample_size_nonneg CHECK (
        sequential_min_sample_size >= 0
    )
);

-- ---------------------------------------------------------------------------
-- experiment_iterations  (bigint from 20260419000003; metric_ids from 20260520000003;
--                          snapshot columns from 20260521000003)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS experiment_iterations (
    id                        UUID          NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    experiment_id             UUID          NOT NULL REFERENCES experiments(id),
    flag_id                   UUID          NOT NULL REFERENCES feature_flags(id),
    iteration_number          INT           NOT NULL,
    started_at                TIMESTAMPTZ   NOT NULL,
    ended_at                  TIMESTAMPTZ,
    metric_ids                UUID[]        NOT NULL DEFAULT '{}',
    guardrail_metric_ids      UUID[]        NOT NULL DEFAULT '{}',
    traffic_allocation        NUMERIC(5,1)  NOT NULL,
    min_sample_size           BIGINT,
    targets_default_rule      BOOLEAN       NOT NULL DEFAULT FALSE,
    pre_period_days           INT           NOT NULL DEFAULT 0,
    unit_context_types        TEXT[]        NOT NULL DEFAULT '{user}',
    default_rule_distribution JSONB,
    -- Sequential testing configuration snapshot at iteration start.
    sequential_testing_enabled BOOLEAN          NOT NULL DEFAULT FALSE,
    sequential_alpha           DOUBLE PRECISION NOT NULL DEFAULT 0.05,
    sequential_tau_squared     DOUBLE PRECISION,
    sequential_min_sample_size BIGINT           NOT NULL DEFAULT 100,
    UNIQUE (experiment_id, iteration_number),
    CONSTRAINT experiment_iterations_pre_period_days_nonneg
        CHECK (pre_period_days >= 0),
    CONSTRAINT experiment_iterations_unit_context_types_nonempty
        CHECK (cardinality(unit_context_types) > 0),
    CONSTRAINT experiment_iterations_sequential_alpha_range CHECK (
        sequential_alpha > 0 AND sequential_alpha < 1
    ),
    CONSTRAINT experiment_iterations_sequential_tau_squared_positive CHECK (
        sequential_tau_squared IS NULL OR sequential_tau_squared > 0
    ),
    CONSTRAINT experiment_iterations_sequential_min_sample_size_nonneg CHECK (
        sequential_min_sample_size >= 0
    )
);

-- ---------------------------------------------------------------------------
-- stats_jobs
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS stats_jobs (
    id            UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    experiment_id UUID,
    status        TEXT        NOT NULL DEFAULT 'pending'
                      CHECK (status IN ('pending', 'running', 'completed', 'failed')),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at    TIMESTAMPTZ,
    completed_at  TIMESTAMPTZ,
    error         TEXT
);

-- ---------------------------------------------------------------------------
-- stats_schedule
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS stats_schedule (
    experiment_id      UUID        NOT NULL REFERENCES experiments(id) PRIMARY KEY,
    last_computed_at   TIMESTAMPTZ,
    next_run_at        TIMESTAMPTZ,
    computation_status TEXT        NOT NULL DEFAULT 'never_computed'
                           CHECK (computation_status IN ('ready', 'computing', 'never_computed')),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ---------------------------------------------------------------------------
-- context_type_registry
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS context_type_registry (
    env_id        UUID        NOT NULL,
    context_type  TEXT        NOT NULL,
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (env_id, context_type)
);

-- ---------------------------------------------------------------------------
-- context_param_registry
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS context_param_registry (
    env_id        UUID        NOT NULL,
    context_type  TEXT        NOT NULL,
    param_key     TEXT        NOT NULL,
    inferred_type TEXT        NOT NULL DEFAULT 'unknown'
                              CHECK (inferred_type IN ('str','int','double','bool','semver','unknown')),
    is_private    BOOLEAN     NOT NULL DEFAULT FALSE,
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (env_id, context_type, param_key)
);

-- Singleton row bumped by triggers whenever experiment_iterations or experiments change;
-- used by the ClickHouse dictionary's invalidate_query.
CREATE TABLE IF NOT EXISTS experiment_iterations_active_audit (
    id         INT PRIMARY KEY CHECK (id = 1),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO experiment_iterations_active_audit (id, updated_at)
    VALUES (1, now())
    ON CONFLICT (id) DO NOTHING;

-- =============================================================================
-- INDEXES
-- =============================================================================

CREATE INDEX IF NOT EXISTS idx_projects_organisation_id
    ON projects(organisation_id);

CREATE INDEX IF NOT EXISTS idx_projects_organisation_active
    ON projects(organisation_id) WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_environments_project_id
    ON environments(project_id);

CREATE INDEX IF NOT EXISTS idx_environments_project_active
    ON environments(project_id) WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_roles_project_id
    ON roles(project_id);

CREATE INDEX IF NOT EXISTS idx_permissions_role_id
    ON permissions(role_id);

CREATE INDEX IF NOT EXISTS idx_user_project_roles_user_id
    ON user_project_roles(user_id);

CREATE INDEX IF NOT EXISTS idx_user_project_roles_project_id
    ON user_project_roles(project_id);

CREATE INDEX IF NOT EXISTS idx_refresh_tokens_user_revoked_expires
    ON refresh_tokens (user_id, revoked_at, expires_at);

CREATE INDEX IF NOT EXISTS idx_sdk_keys_environment_active
    ON sdk_keys(environment_id, is_active);

CREATE INDEX IF NOT EXISTS idx_sdk_keys_key_hash_active
    ON sdk_keys(key_hash, is_active);

CREATE INDEX IF NOT EXISTS idx_feature_flags_project_id
    ON feature_flags(project_id);

CREATE INDEX IF NOT EXISTS idx_feature_flags_default_variant_id
    ON feature_flags(default_variant_id);

CREATE INDEX IF NOT EXISTS idx_feature_flags_project_active
    ON feature_flags(project_id) WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_variants_flag_id
    ON variants(flag_id);

CREATE INDEX IF NOT EXISTS idx_flag_hashing_config_flag_id
    ON flag_hashing_config(flag_id);

CREATE INDEX IF NOT EXISTS idx_feature_flag_rules_flag_id
    ON feature_flag_rules(flag_id);

CREATE INDEX IF NOT EXISTS idx_segments_environment_id
    ON segments(environment_id);

CREATE INDEX IF NOT EXISTS idx_segments_environment_active
    ON segments(environment_id) WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_audit_log_resource
    ON audit_log(resource_type, resource_id);

CREATE INDEX IF NOT EXISTS idx_audit_log_actor_id
    ON audit_log(actor_id);

CREATE INDEX IF NOT EXISTS idx_audit_log_created_at
    ON audit_log(created_at);

CREATE INDEX IF NOT EXISTS idx_event_definitions_environment_id
    ON event_definitions(environment_id);

CREATE INDEX IF NOT EXISTS idx_event_definitions_environment_active
    ON event_definitions(environment_id) WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_event_definitions_env_metric_type_live
    ON event_definitions(environment_id, metric_type) WHERE deleted_at IS NULL;

CREATE UNIQUE INDEX IF NOT EXISTS idx_metric_definitions_env_key_live
    ON metric_definitions(environment_id, key) WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_metric_definitions_environment_id_live
    ON metric_definitions(environment_id) WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_metric_definitions_kind
    ON metric_definitions(environment_id, kind) WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_experiments_env_id
    ON experiments(env_id);

CREATE INDEX IF NOT EXISTS idx_experiments_flag_rule_id
    ON experiments(flag_rule_id);

CREATE INDEX IF NOT EXISTS idx_experiments_flag_id
    ON experiments(flag_id);

CREATE INDEX IF NOT EXISTS idx_experiments_env_active
    ON experiments(env_id) WHERE deleted_at IS NULL;

CREATE UNIQUE INDEX IF NOT EXISTS idx_experiments_one_active_per_flag
    ON experiments (flag_id)
    WHERE status IN ('running', 'paused') AND deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_experiment_iterations_experiment_id
    ON experiment_iterations(experiment_id);

CREATE INDEX IF NOT EXISTS idx_experiment_iterations_flag_id
    ON experiment_iterations(flag_id);

CREATE INDEX IF NOT EXISTS idx_stats_jobs_experiment_id
    ON stats_jobs (experiment_id) WHERE experiment_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_stats_jobs_status
    ON stats_jobs (status);

CREATE INDEX IF NOT EXISTS idx_context_type_registry_env_last
    ON context_type_registry (env_id, last_seen_at DESC);

CREATE INDEX IF NOT EXISTS idx_context_type_registry_last_seen
    ON context_type_registry(last_seen_at);

CREATE INDEX IF NOT EXISTS idx_context_param_registry_env_type_last
    ON context_param_registry (env_id, context_type, last_seen_at DESC);

CREATE INDEX IF NOT EXISTS idx_context_param_registry_last_seen
    ON context_param_registry(last_seen_at);

-- =============================================================================
-- VIEWS
-- =============================================================================

-- Active experiment iterations expanded per unit_context_type; backing source
-- for the ClickHouse experiment_iterations_active dictionary.
CREATE OR REPLACE VIEW v_experiment_iterations_active AS
SELECT
    e.env_id           AS env_id,
    ei.flag_id         AS flag_id,
    e.flag_rule_id     AS matched_rule_id,
    ct                 AS context_type,
    ei.id              AS iteration_id,
    ei.experiment_id   AS experiment_id,
    ei.iteration_number AS iteration_number,
    ei.started_at      AS started_at,
    ei.ended_at        AS ended_at
FROM experiment_iterations ei
JOIN experiments e ON e.id = ei.experiment_id
CROSS JOIN LATERAL UNNEST(ei.unit_context_types) AS ct
WHERE e.status IN ('running', 'paused')
  AND e.deleted_at IS NULL
  AND ei.ended_at IS NULL;

-- =============================================================================
-- FUNCTIONS & TRIGGERS
-- =============================================================================

CREATE OR REPLACE FUNCTION bump_experiment_iterations_active_audit()
RETURNS TRIGGER AS $f$
BEGIN
    UPDATE experiment_iterations_active_audit
       SET updated_at = now()
     WHERE id = 1;
    RETURN NULL;
END;
$f$ LANGUAGE plpgsql;

CREATE OR REPLACE TRIGGER trg_experiments_bump_iter_active_audit
AFTER INSERT OR UPDATE OR DELETE ON experiments
FOR EACH ROW
EXECUTE FUNCTION bump_experiment_iterations_active_audit();

CREATE OR REPLACE TRIGGER trg_iterations_bump_iter_active_audit
AFTER INSERT OR UPDATE OR DELETE ON experiment_iterations
FOR EACH ROW
EXECUTE FUNCTION bump_experiment_iterations_active_audit();
