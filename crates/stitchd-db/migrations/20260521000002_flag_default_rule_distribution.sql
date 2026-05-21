-- Migration 20260521000002: feature_flags.default_rule_distribution
--
-- Adds a JSONB column to `feature_flags` describing the percentage rollout
-- used when the flag is enabled but no custom rule matches (the "default
-- rule" fall-through). NULL preserves today's single-variant behaviour
-- (serve `default_variant_id`); a populated row replaces that with a
-- hashed percentage distribution over the flag's variants.
--
-- Shape of the JSONB document is intentionally not enforced here — the
-- domain layer in `stitchd-core` validates it against
-- `RolloutDistribution { allocations: Map<VariantKey, Percentage> }` with
-- `sum == 100.0` and each percentage in (0, 100]. Storing as JSONB keeps
-- the migration trivial and lets the validator evolve without schema
-- churn.
--
-- Example document:
--   {
--     "allocations": [
--       { "variant_key": "control",  "percentage": 50.0 },
--       { "variant_key": "treatment","percentage": 50.0 }
--     ]
--   }
--
-- Idempotent.

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'feature_flags'
          AND column_name = 'default_rule_distribution'
    ) THEN
        RAISE NOTICE 'flag_default_rule_distribution already applied; skipping';
        RETURN;
    END IF;

    ALTER TABLE feature_flags
        ADD COLUMN default_rule_distribution JSONB NULL;
END $$;
