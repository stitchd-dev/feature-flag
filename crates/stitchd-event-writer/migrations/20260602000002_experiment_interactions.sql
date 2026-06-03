-- =============================================================================
-- experiment_interactions — N-way cross-experiment interaction statistics
-- =============================================================================
--
-- Superseded the pairwise schema in nway_interaction_20260603 (clean cutover —
-- system not live, no backfill). Generalizes the hardcoded experiment_id_a/b
-- pair into an ordered `experiment_ids` array + `interaction_order` (2 or 3) so
-- pairwise and 3-way share one table. Each candidate tuple emits a full
-- hierarchical decomposition: one row per `term`
--   * "main:<exp>"          — a single-experiment main effect
--   * "2way:<a>x<b>"         — a pairwise interaction term
--   * "3way:<a>x<b>x<c>"     — the top-order three-way interaction term
-- `term` encodes the participating experiments, so (env, order, context, metric,
-- term) uniquely identifies a row across ticks.
--
-- cell_stats holds JSON of the N-dimensional variant-tuple cells (see
-- stitchd-stats-service::interaction_compute::CellAggregate). Frequentist columns
-- (interaction_estimate / p_value / df / significant / insufficient_data) and
-- Bayesian columns (bayes_*) coexist per term; insufficient_data rows carry 0.0
-- sentinels for the Frequentist estimate/p_value (ClickHouse Float64 cannot hold
-- NaN) and must never be rendered as significant.
--
-- Engine: ReplacingMergeTree(computed_at) — the 60-min sweep re-inserts each
-- tick; the latest computed_at wins per ORDER BY key. READERS MUST USE `FINAL`
-- (or argMax) to collapse the unmerged window. A 30-day TTL bounds rows lingering
-- from experiments that have since stopped (sweep refreshes live rows hourly).
CREATE TABLE IF NOT EXISTS experiment_interactions
(
    env_id               UUID,
    experiment_ids       Array(UUID),
    interaction_order    UInt8,
    term                 LowCardinality(String),
    context_type         LowCardinality(String),
    metric_key           LowCardinality(String),
    shared_count         UInt64,
    cell_stats           String,
    interaction_estimate Float64,
    p_value              Float64,
    df                   UInt32,
    significant          Bool,
    insufficient_data    Bool DEFAULT false,
    bayes_prob           Float64,
    bayes_expected       Float64,
    bayes_ci_low         Float64,
    bayes_ci_high        Float64,
    computed_at          DateTime64(3, 'UTC')
)
ENGINE = ReplacingMergeTree(computed_at)
ORDER BY (env_id, interaction_order, context_type, metric_key, term)
TTL toDateTime(computed_at) + INTERVAL 30 DAY;
