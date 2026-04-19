-- Migration 20260419000003: events_count_mv
--
-- Materialized view aggregating event counts per (env_id, metric_key, day).
-- The view populates events_count via SummingMergeTree so partial aggregates merge automatically.

CREATE TABLE IF NOT EXISTS events_count
(
    env_id      UUID,
    metric_key  LowCardinality(String),
    day         Date,
    event_count UInt64
)
ENGINE = ReplicatedSummingMergeTree('/clickhouse/tables/{database}/{table}', '{replica}', event_count)
PARTITION BY toYYYYMM(day)
ORDER BY (env_id, metric_key, day);

CREATE MATERIALIZED VIEW IF NOT EXISTS events_count_mv TO events_count AS
SELECT
    env_id,
    metric_key,
    toDate(timestamp) AS day,
    count()           AS event_count
FROM events
GROUP BY env_id, metric_key, toDate(timestamp);
