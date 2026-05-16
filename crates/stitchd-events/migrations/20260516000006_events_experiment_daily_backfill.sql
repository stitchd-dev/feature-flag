-- Migration 20260516000006: events_experiment_daily_backfill
--
-- Backfills events_experiment_daily from the raw events table.
-- Safe to re-run: TRUNCATE ensures idempotency before repopulating.
-- The MV (events_experiment_daily_mv) continues to auto-populate new inserts;
-- this script only handles historical data present at migration time.

TRUNCATE TABLE IF EXISTS events_experiment_daily;

INSERT INTO events_experiment_daily
    (env_id, experiment_id, variant_key, metric_key, day,
     count_state, sum_state, uniq_ctx_state)
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
