-- Migration 20260521000003: experiment_iterations snapshot columns
--
-- `experiment_iterations` snapshots the experiment configuration at the
-- moment a transition into `running` occurs. We extend the snapshot to
-- include the new attribution fields from migration 20260521000001
-- (targets_default_rule, guardrail_metric_ids, pre_period_days,
-- unit_context_types) plus the flag-level fields needed for in-iteration
-- analysis without re-querying the live `experiments` / `feature_flags`
-- rows (which may have been edited after the iteration ended).
--
-- Also denormalises:
--   * flag_id                   — for join hot paths (assignments,
--                                 stats queries).
--   * default_rule_distribution — snapshot of feature_flags row for
--                                 default-rule-bound iterations; used by
--                                 SRM's expected-allocation computation.
--
-- Pre-launch: no live iterations exist (verified in 20260520000003), so
-- safe defaults can be applied without backfill.
--
-- Idempotent.

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'experiment_iterations'
          AND column_name = 'targets_default_rule'
    ) THEN
        RAISE NOTICE 'experiment_iterations_snapshot already applied; skipping';
        RETURN;
    END IF;

    ALTER TABLE experiment_iterations
        ADD COLUMN flag_id                   UUID,
        ADD COLUMN targets_default_rule      BOOLEAN NOT NULL DEFAULT FALSE,
        ADD COLUMN guardrail_metric_ids      UUID[]  NOT NULL DEFAULT '{}',
        ADD COLUMN pre_period_days           INT     NOT NULL DEFAULT 0,
        ADD COLUMN unit_context_types        TEXT[]  NOT NULL DEFAULT '{user}',
        ADD COLUMN default_rule_distribution JSONB   NULL;

    -- Backfill flag_id from the parent experiment row.
    UPDATE experiment_iterations ei
    SET flag_id = e.flag_id
    FROM experiments e
    WHERE ei.experiment_id = e.id
      AND ei.flag_id IS NULL;

    -- After backfill, lock NOT NULL.
    ALTER TABLE experiment_iterations
        ALTER COLUMN flag_id SET NOT NULL,
        ADD CONSTRAINT experiment_iterations_flag_id_fk
            FOREIGN KEY (flag_id) REFERENCES feature_flags(id);

    CREATE INDEX idx_experiment_iterations_flag_id
        ON experiment_iterations(flag_id);

    -- See note in 20260521000001 — use `cardinality()` rather than
    -- `array_length(arr, 1)`, which returns NULL for empty arrays.
    ALTER TABLE experiment_iterations
        ADD CONSTRAINT experiment_iterations_pre_period_days_nonneg
            CHECK (pre_period_days >= 0),
        ADD CONSTRAINT experiment_iterations_unit_context_types_nonempty
            CHECK (cardinality(unit_context_types) > 0);
END $$;
