-- ClickHouse Migration 20260521000005: experiment_results context_type column
--
-- Phase 5 Task 5.5 — extend the `experiment_results` table to carry a
-- `context_type` per row so the analyses produced by the per-context-type
-- result builder (spec §3 — "all analyses are computed independently
-- per context type") persist their attribution unit.
--
-- ## Why CREATE TABLE IF NOT EXISTS appears here
--
-- The canonical DDL for `experiment_results` lives in
-- `crates/stitchd-analytics-service/clickhouse-migrations/0001_experiment_results.sql`
-- (a reference document — not currently registered in any embedded
-- runner). The live event-writer-driven runner already applies CH
-- schema for every other table the workspace consumes (events_v2,
-- experiment_assignments, flag_evaluation_log, …). To keep
-- experiment_results in sync with the rest, this migration:
--
--   1. Idempotently creates the table if it's missing (no-op when the
--      reference DDL was already applied out-of-band).
--   2. Adds the `context_type` column.
--
-- The PRIMARY-KEY / ORDER-BY does not change (CH ReplacingMergeTree
-- primary-key changes aren't supported in-place; rewriting the table
-- would require a CREATE-RENAME pattern). Per the spec, existing rows
-- get the literal default `'user'` so single-context historical results
-- continue to read correctly.

CREATE TABLE IF NOT EXISTS experiment_results
(
    env_id              UUID,
    experiment_id       UUID,
    iteration_id        UUID,
    variant_key         String,
    metric_key          String,
    metric_type         String,
    variant_stats       String,
    frequentist_result  String,
    bayesian_result     String,
    recommendation      String,
    computed_at         DateTime64(3, 'UTC'),
    created_at          DateTime64(3, 'UTC')
)
ENGINE = MergeTree()
ORDER BY (env_id, experiment_id, variant_key, metric_key, iteration_id);

ALTER TABLE experiment_results
    ADD COLUMN IF NOT EXISTS context_type LowCardinality(String) DEFAULT 'user';
