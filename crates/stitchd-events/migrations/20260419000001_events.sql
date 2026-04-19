-- Migration 20260419000001: events
--
-- Raw ingested events table. One row per event, stored in ClickHouse for analytical queries.
-- Partitioned by month for efficient time-range queries and data retention management.
-- contexts stores evaluation-time context pairs as (type, key) tuples.

CREATE TABLE IF NOT EXISTS events
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
ENGINE = ReplicatedMergeTree('/clickhouse/tables/{database}/{table}', '{replica}')
PARTITION BY toYYYYMM(timestamp)
ORDER BY (env_id, metric_key, timestamp);
