-- Migration 20260516000005: events_experiment_daily_mv
--
-- AggregatingMergeTree table + materialized view for experiment-level daily stats.
-- Keys: (env_id, experiment_id, variant_key, metric_key, day)
-- Populated automatically from the `events` table whenever rows are inserted.
--
-- Query with finalizeAggregation:
--   SELECT env_id, experiment_id, variant_key, metric_key, day,
--          finalizeAggregation(count_state) AS count,
--          finalizeAggregation(sum_state)   AS total_sum,
--          finalizeAggregation(uniq_ctx_state) AS unique_contexts
--   FROM events_experiment_daily
--   WHERE env_id = ? AND experiment_id = ?
--   GROUP BY env_id, experiment_id, variant_key, metric_key, day

CREATE TABLE IF NOT EXISTS events_experiment_daily
(
    env_id          UUID,
    experiment_id   String,
    variant_key     String,
    metric_key      LowCardinality(String),
    day             Date,
    count_state     AggregateFunction(count),
    sum_state       AggregateFunction(sum, Float64),
    uniq_ctx_state  AggregateFunction(uniq, String)
)
ENGINE = AggregatingMergeTree()
PARTITION BY toYYYYMM(day)
ORDER BY (env_id, experiment_id, variant_key, metric_key, day);

CREATE MATERIALIZED VIEW IF NOT EXISTS events_experiment_daily_mv TO events_experiment_daily AS
SELECT
    env_id,
    arrayFirst(t -> t.1 = 'experiment', contexts).2                             AS experiment_id,
    arrayFirst(t -> t.1 = 'variant', contexts).2                                AS variant_key,
    metric_key,
    toDate(timestamp)                                                            AS day,
    countState()                                                                 AS count_state,
    sumState(ifNull(coalesce(value_double, CAST(value_int AS Nullable(Float64))), 0.0)) AS sum_state,
    uniqState(arrayFirst(t -> t.2 != '', contexts).2)                           AS uniq_ctx_state
FROM events
WHERE arrayExists(t -> t.1 = 'experiment', contexts)
  AND arrayExists(t -> t.1 = 'variant', contexts)
GROUP BY env_id, experiment_id, variant_key, metric_key, toDate(timestamp);
