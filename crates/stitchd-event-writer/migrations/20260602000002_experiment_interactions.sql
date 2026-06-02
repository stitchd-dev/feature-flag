-- =============================================================================
-- experiment_interactions — pairwise experiment interaction statistics
-- =============================================================================

-- Per-(experiment pair, context, metric) interaction analysis results.
-- cell_stats holds JSON of per-(variant_A x variant_B) cell counts/means.
CREATE TABLE IF NOT EXISTS experiment_interactions
(
    env_id               UUID,
    experiment_id_a      UUID,
    experiment_id_b      UUID,
    context_type         LowCardinality(String),
    metric_key           LowCardinality(String),
    shared_count         UInt64,
    cell_stats           String,
    interaction_estimate Float64,
    p_value              Float64,
    significant          Bool,
    computed_at          DateTime64(3, 'UTC')
)
ENGINE = MergeTree()
ORDER BY (env_id, experiment_id_a, experiment_id_b, context_type, metric_key);
