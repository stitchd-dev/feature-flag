-- Migration 20260520000002: experiment_metrics_cutover
--
-- Pre-launch cutover. Swap `experiments.metric_keys TEXT[]` (raw event-key
-- strings) for `experiments.metric_ids UUID[]` referencing rows in
-- `metric_definitions` (introduced in 20260520000001).
--
-- For each (env_id, distinct event_key) currently referenced by any live
-- experiment, an `aggregation`-kind `metric_definition` is auto-created with
--   key         = 'count_of_' || event_key
--   config      = { event_key, aggregator: 'count' }
-- and the experiment's `metric_ids` column is populated with the matching
-- definition IDs. The legacy `metric_keys` column is then dropped.
--
-- No back-compat is needed (pre-launch). The Rust proto / domain / repo
-- consumers are refactored in Phase 7 Task 7.2; running this migration
-- before that worker lands WILL break experiment-repo queries until the
-- code is updated. That is intentional — cutover is meant to be atomic.
--
-- `experiment_iterations.metric_keys` is a historical snapshot column and
-- is deliberately left untouched. (No live iterations exist pre-launch;
-- the snapshot schema will be addressed alongside the proto refactor if
-- needed.)
--
-- Idempotent: re-running is a no-op once `metric_ids` exists.

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'experiments' AND column_name = 'metric_ids'
    ) THEN
        RAISE NOTICE 'experiment_metrics_cutover already applied; skipping';
        RETURN;
    END IF;

    -- 1) Auto-create metric_definitions for every (env_id, event_key) referenced.
    --    Uses 'count_of_<event_key>' as the deterministic synthetic key so the
    --    follow-up UPDATE can resolve metric_keys -> metric_ids by name.
    --    ON CONFLICT against the live-row partial unique index makes this safe
    --    when a metric with that key already exists in the env.
    INSERT INTO metric_definitions (
        id, environment_id, key, name, description,
        kind, config, goal_direction,
        version, created_at, updated_at
    )
    SELECT DISTINCT
        gen_random_uuid(),
        e.env_id,
        'count_of_' || mk,
        'Count of ' || mk,
        'Auto-created during 20260520000002 cutover from experiments.metric_keys.',
        'aggregation',
        jsonb_build_object('event_key', mk, 'aggregator', 'count'),
        'increase',
        1,
        NOW(),
        NOW()
    FROM experiments e
    CROSS JOIN LATERAL unnest(e.metric_keys) AS mk
    WHERE e.deleted_at IS NULL
    ON CONFLICT (environment_id, key) WHERE deleted_at IS NULL DO NOTHING;

    -- 2) Add the new metric_ids column. Default '{}' for any rows whose
    --    metric_keys was already empty (shouldn't happen given the NOT NULL
    --    constraint, but harmless).
    ALTER TABLE experiments ADD COLUMN metric_ids UUID[] NOT NULL DEFAULT '{}';

    -- 3) Populate metric_ids from the freshly-inserted metric_definitions.
    --    Soft-deleted experiments are intentionally skipped (no FK lookup).
    UPDATE experiments e
    SET metric_ids = (
        SELECT COALESCE(array_agg(m.id), '{}')
        FROM unnest(e.metric_keys) AS mk
        LEFT JOIN metric_definitions m
            ON m.environment_id = e.env_id
            AND m.key = 'count_of_' || mk
            AND m.deleted_at IS NULL
    )
    WHERE e.deleted_at IS NULL;

    -- 4) Drop the legacy column. CASCADE not needed: no view / index / FK
    --    references metric_keys (verified against current schema).
    ALTER TABLE experiments DROP COLUMN metric_keys;
END $$;
