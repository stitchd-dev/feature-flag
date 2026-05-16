-- ClickHouse Migration 003: flag_evaluation_log
--
-- Records every server-side flag evaluation (currently: evaluate_preview calls).
-- One row per context per evaluation call. Private parameter values are masked
-- to "********" before insertion; keys are preserved.
--
-- TTL enforces the universal 90-day data horizon: rows older than 90 days are
-- hard-deleted by ClickHouse background merges.

CREATE TABLE IF NOT EXISTS flag_evaluation_log
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
PARTITION BY toYYYYMM(evaluated_at)
ORDER BY (env_id, flag_id, evaluated_at)
TTL toDateTime(evaluated_at) + INTERVAL 90 DAY DELETE;
