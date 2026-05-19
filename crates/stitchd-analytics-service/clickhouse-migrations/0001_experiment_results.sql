-- ClickHouse Migration 001 (analytics-service): experiment_results
--
-- Stores computed statistical results for each metric in an experiment
-- iteration, partitioned per analytics-service ownership.
--
-- Design notes:
--   - One row per (env_id, experiment_id, variant_key, metric_key, iteration_id) tuple.
--   - Using MergeTree (not ReplacingMergeTree) since writes come from the
--     stats-service batch job and explicit deduplication is handled there via
--     the DISTINCT write path. If upsert semantics are needed later, migrate
--     to ReplacingMergeTree(computed_at).
--   - No partitioning: experiment result volumes are small (one row per variant
--     per metric per iteration) so partition pruning adds no benefit.
--   - String columns for UUIDs are stored as UUID native type for compact
--     storage and fast range scans on the ORDER BY key.
--   - JSONB equivalent in ClickHouse is String (JSON stored as serialized text).
--     variant_stats, frequentist_result, and bayesian_result follow this pattern.
--     Empty optional results are stored as empty string ''.

CREATE TABLE IF NOT EXISTS experiment_results
(
    -- Tenancy / scoping
    env_id              UUID,
    -- Experiment identity
    experiment_id       UUID,
    iteration_id        UUID,
    -- Metric identity
    variant_key         String,
    metric_key          String,
    metric_type         String,
    -- Statistical payload (JSON as String, empty string when absent)
    variant_stats       String,
    frequentist_result  String,
    bayesian_result     String,
    -- Human-readable outcome
    recommendation      String,
    -- Timestamps
    computed_at         DateTime64(3, 'UTC'),
    created_at          DateTime64(3, 'UTC')
)
ENGINE = MergeTree()
ORDER BY (env_id, experiment_id, variant_key, metric_key, iteration_id);
