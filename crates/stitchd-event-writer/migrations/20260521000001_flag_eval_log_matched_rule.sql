-- ClickHouse Migration 20260521000001: flag_evaluation_log experiment-attribution columns
--
-- Two schema changes for the experimentation_full_20260521 attribution
-- pipeline:
--
--   1. Replace `is_disabled Bool` with `targeting_on Bool` (inverted
--      semantics). Flag-service's "is the flag enabled" boolean is the
--      universal feature-flag concept of "targeting on/off"; naming the
--      column `targeting_on` makes the MV filter `WHERE targeting_on`
--      self-documenting and aligns with vendor terminology
--      (LaunchDarkly / Statsig / Optimizely).
--
--   2. Add `matched_rule_id Nullable(UUID)`:
--        * UUID  → a custom rule matched at evaluation time.
--        * NULL  → either (a) the flag fell through to the default rule
--                  (targeting_on=true, no custom rule matched), or
--                  (b) targeting was off (targeting_on=false) — in which
--                  case rule evaluation is skipped entirely.
--      The Phase 4 `experiment_assignments_mv` joins on
--      `(env_id, flag_id, matched_rule_id, context_type)` against the
--      `experiment_iterations_active` dictionary, with
--      `WHERE targeting_on` as a hard filter so disabled-flag evals never
--      become experiment exposures.
--
-- The Bool DEFAULT uses `NOT is_disabled` to derive correct values for
-- existing rows (pre-launch eval-log entries written before this
-- migration). MATERIALIZE COLUMN forces synchronous backfill before the
-- DROP so we don't have a window where the column lacks data.
--
-- Pre-launch: no production analytics queries reference `is_disabled` yet
-- (per `db_optim_20260516` index audit), so the destructive rename is safe.

ALTER TABLE flag_evaluation_log
    ADD COLUMN IF NOT EXISTS targeting_on Bool DEFAULT (NOT is_disabled) AFTER variant_key;

ALTER TABLE flag_evaluation_log
    ADD COLUMN IF NOT EXISTS matched_rule_id Nullable(UUID) AFTER targeting_on;

ALTER TABLE flag_evaluation_log
    MATERIALIZE COLUMN targeting_on SETTINGS mutations_sync = 2;

-- Break the DEFAULT-expression dependency on `is_disabled` so the column can
-- be dropped. After MATERIALIZE the values are persisted, so the DEFAULT
-- doesn't need to compute from `is_disabled` anymore.
ALTER TABLE flag_evaluation_log
    MODIFY COLUMN targeting_on Bool DEFAULT true;

ALTER TABLE flag_evaluation_log
    DROP COLUMN IF EXISTS is_disabled SETTINGS mutations_sync = 2;
