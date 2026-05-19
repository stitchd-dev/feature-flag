-- Migration 20260520000001: events_v2 — add `properties` + `occurred_at`
--
-- Adds two columns to the canonical events_v2 ingestion table to support
-- the events_metrics_20260519 track:
--
--   * `properties Map(String, String)` — arbitrary per-event metadata
--     that the metrics layer can filter on (via JsonLogic where_clause)
--     and aggregate over (via AggregationConfig.on_field).
--   * `occurred_at DateTime64(3, 'UTC')` — wall-clock time the event
--     happened on the client. Distinct from `timestamp` (broker
--     ingestion time) and `ingested_at` (CH server time). Lets the SDK
--     submit events with a real client-side timestamp when buffering or
--     replaying offline data.
--
-- Both columns are nullable in spirit (`properties` defaults to empty
-- map, `occurred_at` defaults to `timestamp` when absent at write time —
-- the SDK / analytics-service handles that in code, not in SQL, because
-- ClickHouse DEFAULTs on ADD COLUMN re-write the whole table). Adding
-- columns without DEFAULT clauses is metadata-only and instant in
-- ClickHouse 24+.

ALTER TABLE events_v2
    ADD COLUMN IF NOT EXISTS properties  Map(String, String),
    ADD COLUMN IF NOT EXISTS occurred_at DateTime64(3, 'UTC');

-- An index on the `properties` map would require ClickHouse skipping
-- indexes or projections — both add write overhead and slow ingestion.
-- The properties column is filtered at scan time inside the experiment
-- aggregation MV (`events_experiment_daily_mv`), and we rely on the
-- partition pruning + ORDER BY for query performance.
--
-- If a specific high-cardinality property becomes a hot filter we will
-- add a `bloom_filter` skip index in a follow-up migration.
