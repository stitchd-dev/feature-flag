-- =============================================================================
-- V1 Baseline — stitchd-event-writer ClickHouse schema (clean cutover 2026-06-09)
--
-- Single consolidated ClickHouse baseline representing the FINAL schema state.
-- The system is not live: this collapses the prior 2026-05-25 CH baseline plus
-- the two post-baseline migrations (experiment_interactions N-way table;
-- flag_evaluation_log.evaluation_id column) into one fresh baseline.
-- No migration path / no back-compat.
--
-- Single canonical `events` table (the legacy duplicate raw-events table is retired).
-- Single canonical `flag_evaluation_log` table (its legacy duplicate is retired).
-- =============================================================================

-- =============================================================================
-- TABLE ENGINES
-- =============================================================================

-- Raw ingested events; weekly-partitioned for short time-range queries.
-- properties and occurred_at support client-side timestamps and metrics filtering.
CREATE TABLE IF NOT EXISTS events
(
    env_id      UUID,
    contexts    Array(Tuple(String, String)),
    metric_key  LowCardinality(String),
    value_bool  Nullable(Bool),
    value_int   Nullable(Int64),
    value_double Nullable(Float64),
    timestamp   DateTime64(3, 'UTC'),
    ingested_at DateTime64(3, 'UTC') DEFAULT now64(),
    properties  Map(String, String),
    occurred_at DateTime64(3, 'UTC')
)
ENGINE = MergeTree()
PARTITION BY toMonday(timestamp)
ORDER BY (env_id, metric_key, timestamp);

-- Mirror of the PostgreSQL event_definitions table for query-time joins.
-- ReplacingMergeTree converges on latest version from PostgreSQL upserts.
CREATE TABLE IF NOT EXISTS metric_definitions
(
    env_id     UUID,
    key        LowCardinality(String),
    value_type Enum8('bool' = 1, 'int' = 2, 'double' = 3),
    updated_at DateTime64(3, 'UTC')
)
ENGINE = ReplicatedReplacingMergeTree('/clickhouse/tables/{database}/{table}', '{replica}', updated_at)
ORDER BY (env_id, key);

-- Aggregate event counts per (env_id, metric_key, day); backing table for events_count_mv.
CREATE TABLE IF NOT EXISTS events_count
(
    env_id      UUID,
    metric_key  LowCardinality(String),
    day         Date,
    event_count UInt64
)
ENGINE = ReplicatedSummingMergeTree('/clickhouse/tables/{database}/{table}', '{replica}', event_count)
PARTITION BY toYYYYMM(day)
ORDER BY (env_id, metric_key, day);

-- Partial-aggregate numeric stats per (env_id, metric_key, day).
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

-- Experiment-level daily aggregate stats for analysis.
CREATE TABLE IF NOT EXISTS events_experiment_daily
(
    env_id         UUID,
    experiment_id  String,
    variant_key    String,
    metric_key     LowCardinality(String),
    day            Date,
    count_state    AggregateFunction(count),
    sum_state      AggregateFunction(sum, Float64),
    uniq_ctx_state AggregateFunction(uniq, String)
)
ENGINE = AggregatingMergeTree()
PARTITION BY toYYYYMM(day)
ORDER BY (env_id, experiment_id, variant_key, metric_key, day);

-- Flag evaluation log for experimentation attribution; 90-day TTL.
-- evaluation_id stamps one UUID per multi-context evaluation bundle so the
-- per-context-type sibling rows can be grouped (experiment_assignments_mv
-- cross-context attribution). Written by EvalLogRow / the SDK eval-log ingest
-- path; non-nullable UUID to match EvalLogRow.evaluation_id.
CREATE TABLE IF NOT EXISTS flag_evaluation_log
(
    env_id          UUID,
    flag_id         UUID,
    flag_key        String,
    variant_key     String,
    targeting_on    Bool DEFAULT true,
    matched_rule_id Nullable(UUID),
    evaluated_at    DateTime64(3, 'UTC'),
    context_type    String,
    context_key     String,
    params_json     String,
    evaluation_id   UUID DEFAULT generateUUIDv4()
)
ENGINE = MergeTree()
PARTITION BY toMonday(evaluated_at)
ORDER BY (env_id, flag_id, evaluated_at)
TTL toDateTime(evaluated_at) + INTERVAL 90 DAY DELETE;

-- First-exposure experiment assignments; ReplacingMergeTree(_version) keeps
-- earliest assignment per (experiment_id, iteration_id, context_type, context_key).
CREATE TABLE IF NOT EXISTS experiment_assignments
(
    experiment_id   UUID,
    iteration_id    UUID,
    env_id          UUID,
    flag_id         UUID,
    matched_rule_id Nullable(UUID),
    context_type    LowCardinality(String),
    context_key     String,
    variant_key     String,
    assigned_at     DateTime64(3, 'UTC'),
    _version        Int64
)
ENGINE = ReplacingMergeTree(_version)
PARTITION BY toYYYYMM(assigned_at)
ORDER BY (experiment_id, iteration_id, context_type, context_key)
TTL toDateTime(assigned_at) + INTERVAL 180 DAY DELETE;

-- Per-variant, per-metric statistical results for experiment iterations.
-- sequential_result holds the per-variant always-valid p / anytime-CI JSON blob
-- (seqtest_20260603) alongside frequentist_result / bayesian_result.
CREATE TABLE IF NOT EXISTS experiment_results
(
    env_id             UUID,
    experiment_id      UUID,
    iteration_id       UUID,
    variant_key        String,
    metric_key         String,
    metric_type        String,
    variant_stats      String,
    frequentist_result String,
    bayesian_result    String,
    recommendation     String,
    computed_at        DateTime64(3, 'UTC'),
    created_at         DateTime64(3, 'UTC'),
    context_type       LowCardinality(String) DEFAULT 'user'
)
ENGINE = MergeTree()
ORDER BY (env_id, experiment_id, variant_key, metric_key, iteration_id);

-- N-way cross-experiment interaction statistics. Each candidate tuple emits a
-- full hierarchical decomposition (one row per `term`: main / 2way / 3way…).
-- experiment_ids is REQUIRED in the sort key so a lower-order term shared by
-- multiple candidate tuples does not collide under ReplacingMergeTree FINAL.
-- ReplacingMergeTree(computed_at): the 60-min sweep re-inserts each tick;
-- READERS MUST USE `FINAL` (or argMax). 30-day TTL bounds stale rows.
CREATE TABLE IF NOT EXISTS experiment_interactions
(
    env_id               UUID,
    experiment_ids       Array(UUID),
    interaction_order    UInt8,
    term                 String,
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
ORDER BY (env_id, interaction_order, experiment_ids, context_type, metric_key, term)
TTL toDateTime(computed_at) + INTERVAL 30 DAY;

-- =============================================================================
-- DICTIONARIES (must precede any Materialized View that references them)
-- =============================================================================

-- Active experiment iterations keyed on (env_id, flag_id, matched_rule_id, context_type).
-- Sourced from PG view v_experiment_iterations_active; refreshed every 30-60 seconds.
CREATE DICTIONARY IF NOT EXISTS experiment_iterations_active
(
    env_id          UUID,
    flag_id         UUID,
    matched_rule_id Nullable(UUID),
    context_type    String,
    iteration_id    UUID,
    experiment_id   UUID,
    iteration_number Int32,
    started_at      DateTime64(3, 'UTC'),
    ended_at        Nullable(DateTime64(3, 'UTC'))
)
PRIMARY KEY env_id, flag_id, matched_rule_id, context_type
SOURCE(POSTGRESQL(
    port 5432
    host 'host.docker.internal'
    user 'stitchd'
    password 'stitchd'
    db 'stitchd'
    table 'public.v_experiment_iterations_active'
    invalidate_query 'SELECT updated_at FROM public.experiment_iterations_active_audit WHERE id = 1'
))
LIFETIME(MIN 30 MAX 60)
LAYOUT(COMPLEX_KEY_HASHED());

-- =============================================================================
-- MATERIALIZED VIEWS
-- =============================================================================

-- Aggregates event counts per (env_id, metric_key, day) into events_count.
CREATE MATERIALIZED VIEW IF NOT EXISTS events_count_mv TO events_count AS
SELECT
    env_id,
    metric_key,
    toDate(timestamp) AS day,
    count()           AS event_count
FROM events
GROUP BY env_id, metric_key, toDate(timestamp);

-- Aggregates numeric event values (sum/count/percentiles) into events_numeric.
CREATE MATERIALIZED VIEW IF NOT EXISTS events_numeric_mv TO events_numeric AS
SELECT
    env_id,
    metric_key,
    toDate(timestamp)                                                                                    AS day,
    sumState(assumeNotNull(coalesce(value_double, CAST(value_int AS Nullable(Float64)))))                AS value_sum,
    countState()                                                                                          AS value_count,
    quantileState(0.5)(assumeNotNull(coalesce(value_double, CAST(value_int AS Nullable(Float64)))))      AS value_p50,
    quantileState(0.95)(assumeNotNull(coalesce(value_double, CAST(value_int AS Nullable(Float64)))))     AS value_p95,
    quantileState(0.99)(assumeNotNull(coalesce(value_double, CAST(value_int AS Nullable(Float64)))))     AS value_p99
FROM events
WHERE value_double IS NOT NULL OR value_int IS NOT NULL
GROUP BY env_id, metric_key, toDate(timestamp);

-- Aggregates experiment-level daily stats into events_experiment_daily.
CREATE MATERIALIZED VIEW IF NOT EXISTS events_experiment_daily_mv TO events_experiment_daily AS
SELECT
    env_id,
    arrayFirst(t -> t.1 = 'experiment', contexts).2                                    AS experiment_id,
    arrayFirst(t -> t.1 = 'variant', contexts).2                                       AS variant_key,
    metric_key,
    toDate(timestamp)                                                                   AS day,
    countState()                                                                        AS count_state,
    sumState(ifNull(coalesce(value_double, CAST(value_int AS Nullable(Float64))), 0.0)) AS sum_state,
    uniqState(arrayFirst(t -> t.2 != '', contexts).2)                                  AS uniq_ctx_state
FROM events
WHERE arrayExists(t -> t.1 = 'experiment', contexts)
  AND arrayExists(t -> t.1 = 'variant', contexts)
GROUP BY env_id, experiment_id, variant_key, metric_key, toDate(timestamp);

-- Derives first-exposure experiment assignments from flag_evaluation_log rows
-- where targeting is on and the eval matches an active experiment iteration.
CREATE MATERIALIZED VIEW IF NOT EXISTS experiment_assignments_mv TO experiment_assignments AS
SELECT
    dictGet('experiment_iterations_active', 'experiment_id',
            (e.env_id, e.flag_id, e.matched_rule_id, e.context_type))  AS experiment_id,
    dictGet('experiment_iterations_active', 'iteration_id',
            (e.env_id, e.flag_id, e.matched_rule_id, e.context_type))  AS iteration_id,
    e.env_id                                                            AS env_id,
    e.flag_id                                                           AS flag_id,
    e.matched_rule_id                                                   AS matched_rule_id,
    CAST(e.context_type AS LowCardinality(String))                      AS context_type,
    e.context_key                                                       AS context_key,
    e.variant_key                                                       AS variant_key,
    e.evaluated_at                                                      AS assigned_at,
    -toUnixTimestamp64Milli(e.evaluated_at)                             AS _version
FROM flag_evaluation_log AS e
WHERE e.targeting_on = true
  AND dictHas('experiment_iterations_active',
              (e.env_id, e.flag_id, e.matched_rule_id, e.context_type));
