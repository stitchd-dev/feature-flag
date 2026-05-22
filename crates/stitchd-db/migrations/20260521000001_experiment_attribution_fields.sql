-- Migration 20260521000001: experiment attribution fields
--
-- Extends the `experiments` table for eval-log-based first-exposure
-- attribution + flag-level locking + per-context-type analysis:
--
--   * flag_id              — denormalised for whole-flag lock + per-flag
--                            uniqueness; always populated (derived from
--                            flag_rule_id when set, otherwise supplied
--                            directly for default-rule-bound experiments).
--   * flag_rule_id         — now NULLABLE. NULL ⇒ experiment binds to the
--                            flag's default-rule fallthrough path.
--   * targets_default_rule — boolean half of the XOR (flag_rule_id IS NOT
--                            NULL) XOR (targets_default_rule = TRUE).
--   * guardrail_metric_ids — UUID[] of guardrail metric_definitions.
--   * pre_period_days      — INT (>= 0), used by CUPED variance reduction.
--                            Default 0 = CUPED off.
--   * unit_context_types   — TEXT[], analysis units for this experiment
--                            (e.g. {user} or {user, account}). Filtered
--                            against context_type_registry by the gateway.
--
-- Uniqueness moves from `(flag_rule_id) WHERE running/paused` (one active
-- experiment per rule) to `(flag_id) WHERE running/paused` (one active
-- experiment per flag), matching the new whole-flag-lock semantics in the
-- experimentation_full_20260521 spec.
--
-- Idempotent — guarded on `targets_default_rule` column presence.

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'experiments' AND column_name = 'targets_default_rule'
    ) THEN
        RAISE NOTICE 'experiment_attribution_fields already applied; skipping';
        RETURN;
    END IF;

    -- 1) flag_id — denormalised from flag_rule_id where available.
    --    We add NULLABLE first, backfill, then enforce NOT NULL.
    ALTER TABLE experiments ADD COLUMN flag_id UUID REFERENCES feature_flags(id);

    UPDATE experiments e
    SET flag_id = r.flag_id
    FROM feature_flag_rules r
    WHERE e.flag_rule_id = r.id
      AND e.flag_id IS NULL;

    -- Any rows still NULL would be pre-existing default-rule-bound rows
    -- (there are none today). Enforce NOT NULL.
    ALTER TABLE experiments ALTER COLUMN flag_id SET NOT NULL;

    CREATE INDEX idx_experiments_flag_id ON experiments(flag_id);

    -- 2) flag_rule_id becomes NULLABLE for default-rule-bound experiments.
    ALTER TABLE experiments ALTER COLUMN flag_rule_id DROP NOT NULL;

    -- 3) New columns.
    ALTER TABLE experiments
        ADD COLUMN targets_default_rule BOOLEAN NOT NULL DEFAULT FALSE,
        ADD COLUMN guardrail_metric_ids UUID[] NOT NULL DEFAULT '{}',
        ADD COLUMN pre_period_days INT NOT NULL DEFAULT 0,
        ADD COLUMN unit_context_types TEXT[] NOT NULL DEFAULT '{user}';

    -- 4) XOR CHECK: exactly one of {flag_rule_id, targets_default_rule}.
    ALTER TABLE experiments
        ADD CONSTRAINT experiments_rule_xor_default CHECK (
            (flag_rule_id IS NOT NULL)::int + targets_default_rule::int = 1
        );

    -- 5) Defensive constraints.
    -- NOTE: `array_length(arr, 1)` returns NULL for empty arrays in
    -- PostgreSQL (not 0), and a CHECK that evaluates to NULL is treated
    -- as passing. Use `cardinality()` instead — it returns 0 for empty
    -- arrays and the comparison evaluates to FALSE as intended.
    ALTER TABLE experiments
        ADD CONSTRAINT experiments_pre_period_days_nonneg CHECK (pre_period_days >= 0),
        ADD CONSTRAINT experiments_unit_context_types_nonempty CHECK (
            cardinality(unit_context_types) > 0
        );

    -- 6) Uniqueness: one running/paused experiment per FLAG (not per rule).
    --    The old per-rule index is replaced because the whole-flag-lock
    --    semantics already preclude multiple concurrent experiments on
    --    different rules of the same flag.
    DROP INDEX IF EXISTS idx_experiments_one_active_per_rule;

    CREATE UNIQUE INDEX idx_experiments_one_active_per_flag
        ON experiments (flag_id)
        WHERE status IN ('running', 'paused')
          AND deleted_at IS NULL;
END $$;
