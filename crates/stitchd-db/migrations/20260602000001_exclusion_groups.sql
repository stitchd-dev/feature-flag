-- =============================================================================
-- Exclusion groups (mutual-exclusion buckets across experiments) [P1.T3]
-- Adds the exclusion_groups table plus group-bucket columns on experiments and
-- snapshot columns on experiment_iterations. Purely additive.
-- =============================================================================

-- ---------------------------------------------------------------------------
-- exclusion_groups
--   salt is immutable (random per group, set at creation) and used for the
--   group-level bucketing hash.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS exclusion_groups (
    id          UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    env_id      UUID        NOT NULL REFERENCES environments(id),
    name        TEXT        NOT NULL,
    description TEXT,
    salt        TEXT        NOT NULL,
    version     INTEGER     NOT NULL DEFAULT 1,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at  TIMESTAMPTZ
);

-- ---------------------------------------------------------------------------
-- experiments — group assignment + group-bucket window (basis points 0..10000)
-- ---------------------------------------------------------------------------
ALTER TABLE experiments
    ADD COLUMN IF NOT EXISTS exclusion_group_id UUID REFERENCES exclusion_groups(id);

ALTER TABLE experiments
    ADD COLUMN IF NOT EXISTS group_bucket_lo INTEGER;

ALTER TABLE experiments
    ADD COLUMN IF NOT EXISTS group_bucket_hi INTEGER;

-- Both bucket bounds are null together, or both set with 0 <= lo < hi <= 10000.
ALTER TABLE experiments
    DROP CONSTRAINT IF EXISTS experiments_group_bucket_range;
ALTER TABLE experiments
    ADD CONSTRAINT experiments_group_bucket_range CHECK (
        (group_bucket_lo IS NULL AND group_bucket_hi IS NULL)
        OR (
            group_bucket_lo IS NOT NULL AND group_bucket_hi IS NOT NULL
            AND group_bucket_lo >= 0
            AND group_bucket_lo < group_bucket_hi
            AND group_bucket_hi <= 10000
        )
    );

-- ---------------------------------------------------------------------------
-- experiment_iterations — frozen snapshot of group assignment + bucket window
-- ---------------------------------------------------------------------------
ALTER TABLE experiment_iterations
    ADD COLUMN IF NOT EXISTS exclusion_group_id UUID;

ALTER TABLE experiment_iterations
    ADD COLUMN IF NOT EXISTS group_bucket_lo INTEGER;

ALTER TABLE experiment_iterations
    ADD COLUMN IF NOT EXISTS group_bucket_hi INTEGER;

-- =============================================================================
-- INDEXES
-- =============================================================================

-- One live group name per environment (matches baseline soft-delete index style).
CREATE UNIQUE INDEX IF NOT EXISTS idx_exclusion_groups_env_name_live
    ON exclusion_groups (env_id, name) WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_exclusion_groups_env_active
    ON exclusion_groups (env_id) WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_experiments_exclusion_group_id
    ON experiments (exclusion_group_id);
