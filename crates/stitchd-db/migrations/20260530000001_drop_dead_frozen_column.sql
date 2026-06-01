-- Drop the dead `feature_flag_rules.frozen` column.
--
-- PROP-001 of domain_boundaries_20260530. The per-rule `frozen` boolean was
-- superseded by the whole-flag lock model (`is_flag_locked`, derived from an
-- experiment's running/paused status — see stitchd-flag-service::flag_lock).
-- Nothing in production ever read `frozen`; `apply_transition` only wrote it.
-- The write path and its test assertions are removed in the same batch.
--
-- Idempotent: only drops if present.
ALTER TABLE feature_flag_rules DROP COLUMN IF EXISTS frozen;
