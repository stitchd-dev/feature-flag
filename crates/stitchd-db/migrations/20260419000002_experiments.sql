-- Migration 20260419000002: experiments
--
-- Adds the Experimentation Module tables:
--   - frozen column on feature_flag_rules (prevents rule mutation while an experiment is active)
--   - experiments: one experiment ties a flag rule to a controlled A/B test
--   - experiment_iterations: immutable snapshot of each time an experiment is started

-- ---------------------------------------------------------------------------
-- feature_flag_rules: add frozen column
-- ---------------------------------------------------------------------------
-- When frozen = TRUE, the rule must not be mutated while an experiment is
-- running or paused against it.
ALTER TABLE feature_flag_rules
    ADD COLUMN frozen BOOLEAN NOT NULL DEFAULT FALSE;

-- ---------------------------------------------------------------------------
-- experiments
-- ---------------------------------------------------------------------------
CREATE TABLE experiments (
    id                  UUID          NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    env_id              UUID          NOT NULL REFERENCES environments(id),
    flag_rule_id        UUID          NOT NULL REFERENCES feature_flag_rules(id),
    name                TEXT          NOT NULL,
    description         TEXT,
    hypothesis          TEXT,
    status              TEXT          NOT NULL
                            CHECK (status IN ('draft', 'running', 'paused', 'stopped')),
    metric_keys         TEXT[]        NOT NULL,
    traffic_allocation  NUMERIC(5,1)  NOT NULL DEFAULT 100.0,
    min_sample_size     INT,
    scheduled_start_at  TIMESTAMPTZ,
    scheduled_end_at    TIMESTAMPTZ,
    version             INT           NOT NULL DEFAULT 1,
    created_at          TIMESTAMPTZ   NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ   NOT NULL DEFAULT now(),
    deleted_at          TIMESTAMPTZ
);

CREATE INDEX idx_experiments_env_id ON experiments(env_id);
CREATE INDEX idx_experiments_flag_rule_id ON experiments(flag_rule_id);

-- Partial unique index: at most one active (running or paused) experiment per
-- flag rule at a time (soft-deleted rows are excluded).
CREATE UNIQUE INDEX idx_experiments_one_active_per_rule
    ON experiments (flag_rule_id)
    WHERE status IN ('running', 'paused')
      AND deleted_at IS NULL;

-- ---------------------------------------------------------------------------
-- experiment_iterations
-- ---------------------------------------------------------------------------
-- Each time an experiment transitions to 'running' a new iteration is created,
-- snapshotting the metric/traffic configuration at that point in time.
CREATE TABLE experiment_iterations (
    id                  UUID          NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    experiment_id       UUID          NOT NULL REFERENCES experiments(id),
    iteration_number    INT           NOT NULL,
    started_at          TIMESTAMPTZ   NOT NULL,
    ended_at            TIMESTAMPTZ,                        -- NULL while the iteration is active
    metric_keys         TEXT[]        NOT NULL,             -- snapshot
    traffic_allocation  NUMERIC(5,1)  NOT NULL,             -- snapshot
    min_sample_size     INT,                                -- snapshot
    UNIQUE (experiment_id, iteration_number)
);

CREATE INDEX idx_experiment_iterations_experiment_id ON experiment_iterations(experiment_id);
