-- Migration 20260419000004: events_numeric_mv
--
-- Materialized view computing sum/avg/p50/p95/p99 per (env_id, metric_key, day).
-- Only rows with a numeric value (value_int or value_double) are included.
-- Uses AggregatingMergeTree to store partial aggregate states that merge correctly.
--
-- Query with merge functions:
--   SELECT env_id, metric_key, day,
--          sumMerge(value_sum), countMerge(value_count),
--          quantileMerge(0.5)(value_p50), quantileMerge(0.95)(value_p95), quantileMerge(0.99)(value_p99)
--   FROM events_numeric
--   GROUP BY env_id, metric_key, day

CREATE TABLE IF NOT EXISTS events_numeric
(
    env_id      UUID,
    metric_key  LowCardinality(String),
    day         Date,
    value_sum   AggregateFunction(sum, Float64),
    value_count AggregateFunction(count, UInt8),
    value_p50   AggregateFunction(quantile(0.5), Float64),
    value_p95   AggregateFunction(quantile(0.95), Float64),
    value_p99   AggregateFunction(quantile(0.99), Float64)
)
ENGINE = ReplicatedAggregatingMergeTree('/clickhouse/tables/{database}/{table}', '{replica}')
PARTITION BY toYYYYMM(day)
ORDER BY (env_id, metric_key, day);

CREATE MATERIALIZED VIEW IF NOT EXISTS events_numeric_mv TO events_numeric AS
SELECT
    env_id,
    metric_key,
    toDate(timestamp)                                                                                                AS day,
    sumState(assumeNotNull(coalesce(value_double, CAST(value_int AS Nullable(Float64)))))                          AS value_sum,
    countState()                                                                                                     AS value_count,
    quantileState(0.5)(assumeNotNull(coalesce(value_double, CAST(value_int AS Nullable(Float64)))))                AS value_p50,
    quantileState(0.95)(assumeNotNull(coalesce(value_double, CAST(value_int AS Nullable(Float64)))))               AS value_p95,
    quantileState(0.99)(assumeNotNull(coalesce(value_double, CAST(value_int AS Nullable(Float64)))))               AS value_p99
FROM events
WHERE value_double IS NOT NULL OR value_int IS NOT NULL
GROUP BY env_id, metric_key, toDate(timestamp);
