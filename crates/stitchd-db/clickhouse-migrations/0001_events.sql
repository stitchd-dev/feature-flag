-- ClickHouse Migration 001: events
--
-- Stores raw metric events submitted by SDK clients.
-- Partitioned by month for efficient time-range queries.
-- Ordered by (environment_id, metric_key, occurred_at) to support
-- per-environment metric aggregation scans.

CREATE TABLE IF NOT EXISTS events
(
    event_id              UUID,
    environment_id        UUID,
    context_type          String,
    context_key           String,
    metric_key            String,
    metric_value_bool     Nullable(UInt8),
    metric_value_int      Nullable(Int64),
    metric_value_double   Nullable(Float64),
    occurred_at           DateTime64(3)
)
ENGINE = MergeTree()
PARTITION BY toYYYYMM(occurred_at)
ORDER BY (environment_id, metric_key, occurred_at);
