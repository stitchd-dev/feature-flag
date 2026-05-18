-- Migration 20260516000007: events_v2
--
-- Replaces the monthly-partitioned events table with weekly partitions.
-- Weekly partitions via toMonday() reduce partition scan overhead for
-- short time-range queries (< 7 days), which are the common case.
--
-- Steps:
-- 1. Create events_v2 with PARTITION BY toMonday(timestamp)
-- 2. Backfill all existing events (idempotent: guarded by migration runner)
-- 3. The RENAME TABLE events -> events_old + events_v2 -> events is a
--    manual production step run after confirming row counts match.
--
-- After manual rename, update all references from events_v2 → events.

CREATE TABLE IF NOT EXISTS events_v2
(
    env_id       UUID,
    contexts     Array(Tuple(String, String)),
    metric_key   LowCardinality(String),
    value_bool   Nullable(Bool),
    value_int    Nullable(Int64),
    value_double Nullable(Float64),
    timestamp    DateTime64(3, 'UTC'),
    ingested_at  DateTime64(3, 'UTC') DEFAULT now64()
)
ENGINE = MergeTree()
PARTITION BY toMonday(timestamp)
ORDER BY (env_id, metric_key, timestamp);

INSERT INTO events_v2
SELECT env_id, contexts, metric_key, value_bool, value_int, value_double, timestamp, ingested_at
FROM events;
