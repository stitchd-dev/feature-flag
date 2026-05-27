-- ClickHouse Migration 001: events
--
-- Canonical ingestion table for SDK metric events.
-- Partitioned weekly (toMonday) for efficient rolling-window queries.
-- Ordered by (env_id, metric_key, timestamp) to support per-env metric scans.

CREATE TABLE IF NOT EXISTS events
(
    env_id       UUID,
    contexts     Array(Tuple(String, String)),
    metric_key   LowCardinality(String),
    value_bool   Nullable(Bool),
    value_int    Nullable(Int64),
    value_double Nullable(Float64),
    timestamp    DateTime64(3, 'UTC'),
    ingested_at  DateTime64(3, 'UTC') DEFAULT now64(),
    properties   Map(String, String),
    occurred_at  DateTime64(3, 'UTC')
)
ENGINE = MergeTree()
PARTITION BY toMonday(timestamp)
ORDER BY (env_id, metric_key, timestamp);
