-- ClickHouse Migration 002: experiment_assignments
--
-- Records which variant each context was assigned to in an experiment.
-- ReplacingMergeTree deduplicates on (experiment_id, context_key) so
-- re-assignments overwrite stale entries during background merges.

CREATE TABLE IF NOT EXISTS experiment_assignments
(
    experiment_id  UUID,
    flag_id        UUID,
    context_key    String,
    variant_key    String,
    assigned_at    DateTime64(3)
)
ENGINE = ReplacingMergeTree(assigned_at)
ORDER BY (experiment_id, context_key);
