-- =============================================================================
-- Bandit foundation schema (bandit_20260608, Phase 1 Task 1). Purely additive.
--
-- Adds:
--   * experiments / experiment_iterations  — experiment_mode + bandit_config
--     (and bandit_campaign_id on experiments, FK wired to bandit_campaigns)
--   * bandit_campaigns       — operator-configured autonomous optimization campaign
--   * bandit_allocation_runs — append-only per-tick reallocation / lifecycle history
--                              (mirrors scheduled_change_runs)
-- =============================================================================

-- ---------------------------------------------------------------------------
-- bandit_campaigns
--   An operator-configured-once construct that auto-creates successive
--   experiment iterations (on convergence / drift) without per-run manual
--   creation. config holds max_iterations, drift thresholds, variant-discovery
--   policy and budget caps.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS bandit_campaigns (
    id                 UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    environment_id     UUID        NOT NULL REFERENCES environments(id),
    flag_id            UUID        NOT NULL REFERENCES feature_flags(id),
    name               TEXT        NOT NULL,
    config             JSONB       NOT NULL,
    status             TEXT        NOT NULL DEFAULT 'active'
                       CONSTRAINT chk_bandit_campaigns_status
                       CHECK (status IN ('active', 'paused', 'completed', 'cancelled')),
    iterations_spawned INT         NOT NULL DEFAULT 0,
    version            BIGINT      NOT NULL DEFAULT 0,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_bandit_campaigns_environment_id
    ON bandit_campaigns (environment_id);
CREATE INDEX IF NOT EXISTS idx_bandit_campaigns_flag_id
    ON bandit_campaigns (flag_id);

-- ---------------------------------------------------------------------------
-- experiments  — bandit mode + config + campaign linkage
-- ---------------------------------------------------------------------------
ALTER TABLE experiments
    ADD COLUMN IF NOT EXISTS experiment_mode TEXT NOT NULL DEFAULT 'fixed'
        CONSTRAINT chk_experiments_experiment_mode
        CHECK (experiment_mode IN ('fixed', 'bandit'));

ALTER TABLE experiments
    ADD COLUMN IF NOT EXISTS bandit_config JSONB;

ALTER TABLE experiments
    ADD COLUMN IF NOT EXISTS bandit_campaign_id UUID REFERENCES bandit_campaigns(id);

CREATE INDEX IF NOT EXISTS idx_experiments_bandit_campaign_id
    ON experiments (bandit_campaign_id) WHERE bandit_campaign_id IS NOT NULL;

-- ---------------------------------------------------------------------------
-- experiment_iterations  — bandit config snapshot at iteration start
-- ---------------------------------------------------------------------------
ALTER TABLE experiment_iterations
    ADD COLUMN IF NOT EXISTS bandit_config JSONB;

-- ---------------------------------------------------------------------------
-- bandit_allocation_runs
--   Append-only per-tick reallocation / autonomous-lifecycle history for a
--   bandit experiment iteration. Mirrors scheduled_change_runs.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS bandit_allocation_runs (
    id             UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    experiment_id  UUID        NOT NULL REFERENCES experiments(id),
    iteration_id   UUID        REFERENCES experiment_iterations(id),
    fired_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    action         TEXT        NOT NULL
                   CONSTRAINT chk_bandit_allocation_runs_action
                   CHECK (action IN ('reallocate', 'commit', 'rollout', 'spawn_iteration', 'skip')),
    old_allocation JSONB,
    new_allocation JSONB,
    outcome        TEXT        NOT NULL
                   CONSTRAINT chk_bandit_allocation_runs_outcome
                   CHECK (outcome IN ('applied', 'skipped', 'failed')),
    -- Free-text reason / error detail (e.g. flag-locked skip reason).
    detail         TEXT
);

CREATE INDEX IF NOT EXISTS idx_bandit_allocation_runs_experiment_iteration_fired
    ON bandit_allocation_runs (experiment_id, iteration_id, fired_at);
