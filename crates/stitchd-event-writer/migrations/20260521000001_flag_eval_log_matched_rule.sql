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
--
-- ── Fresh-deploy guard ──────────────────────────────────────────────────
-- Idempotent `CREATE TABLE IF NOT EXISTS` for the v2 schema (weekly
-- `toMonday()` partitions + 90-day TTL — matches
-- `crates/stitchd-db/clickhouse-migrations/0004_flag_evaluation_log_v2.sql`).
--
-- On existing deploys this is a no-op (the table already exists from the
-- pre-runner manual DDL flow). On fresh deploys (CI, new dev boxes) it
-- creates the table so the subsequent `ALTER ... ADD COLUMN` statements
-- have something to alter. Without this CI fails with `UNKNOWN_TABLE`
-- because the original `0003_flag_evaluation_log.sql` lives in the
-- reference-only `crates/stitchd-db/clickhouse-migrations/` directory
-- and is NOT wired into the embedded migration runner.
--
-- The table is created with the OLD column shape (`is_disabled Bool`)
-- because the rename to `targeting_on` happens immediately below. The
-- IF NOT EXISTS clause means existing deploys (with their actual data)
-- are untouched.
CREATE TABLE IF NOT EXISTS flag_evaluation_log
(
    env_id        UUID,
    flag_id       UUID,
    flag_key      String,
    variant_key   String,
    is_disabled   Bool,
    evaluated_at  DateTime64(3, 'UTC'),
    context_type  String,
    context_key   String,
    params_json   String
)
ENGINE = MergeTree()
PARTITION BY toMonday(evaluated_at)
ORDER BY (env_id, flag_id, evaluated_at)
TTL toDateTime(evaluated_at) + INTERVAL 90 DAY DELETE;

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
