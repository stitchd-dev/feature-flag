-- ClickHouse Migration 004: flag_evaluation_log_v2
--
-- Replaces monthly partitions with weekly partitions (toMonday).
-- Short time-range eval-stats queries (< 7 days) are the common path and
-- benefit from finer partition pruning.
--
-- TTL is preserved: rows older than 90 days are hard-deleted by background merges.
--
-- Manual production steps after applying this migration:
--   1. Verify row counts match:
--        SELECT count() FROM flag_evaluation_log;
--        SELECT count() FROM flag_evaluation_log_v2;
--   2. Rename tables:
--        RENAME TABLE flag_evaluation_log TO flag_evaluation_log_old;
--        RENAME TABLE flag_evaluation_log_v2 TO flag_evaluation_log;
--   3. Drop backup after confirming production health:
--        DROP TABLE IF EXISTS flag_evaluation_log_old;

CREATE TABLE IF NOT EXISTS flag_evaluation_log_v2
(
    env_id        UUID,
    flag_id       UUID,
    flag_key      String,
    variant_key   String,
    is_disabled   Bool,
    evaluated_at  DateTime64(3, 'UTC'),
    context_type  String,
    context_key   String,
    params_json   String
)
ENGINE = MergeTree()
PARTITION BY toMonday(evaluated_at)
ORDER BY (env_id, flag_id, evaluated_at)
TTL toDateTime(evaluated_at) + INTERVAL 90 DAY DELETE;

INSERT INTO flag_evaluation_log_v2
SELECT env_id, flag_id, flag_key, variant_key, is_disabled, evaluated_at,
       context_type, context_key, params_json
FROM flag_evaluation_log;
