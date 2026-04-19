-- Migration 20260419000002: metric_definitions
--
-- Mirror of the PostgreSQL event_definitions table for query-time joins in ClickHouse.
-- Uses ReplacingMergeTree so updates from PostgreSQL upserts converge to the latest version.

CREATE TABLE IF NOT EXISTS metric_definitions
(
    env_id     UUID,
    key        LowCardinality(String),
    value_type Enum8('bool' = 1, 'int' = 2, 'double' = 3),
    updated_at DateTime64(3, 'UTC')
)
ENGINE = ReplicatedReplacingMergeTree('/clickhouse/tables/{database}/{table}', '{replica}', updated_at)
ORDER BY (env_id, key);
