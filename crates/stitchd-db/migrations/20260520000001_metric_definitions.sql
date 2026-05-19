-- Migration 20260520000001: metric_definitions
--
-- Composable metric definitions scoped per environment. Three primitive
-- kinds are supported (the `kind` CHECK constraint matches the serde tags
-- of stitchd-core::metric::MetricKind):
--
--   * aggregation — count/sum/avg/percentile/uniq over a single event
--     stream; config carries event_key + aggregator + optional on_field
--     + optional JsonLogic where_clause
--   * ratio — numerator_metric_id / denominator_metric_id derived metric
--     with a min_denominator floor for statistical validity
--   * funnel — ordered steps[] with window_seconds + count_repeats flag,
--     backed by ClickHouse windowFunnel at the stats-service layer
--
-- The kind-specific config is stored as a single JSONB column rather than
-- normalised tables; cross-metric references (ratio → aggregations) are
-- resolved by the stats-service at compute time, so referential integrity
-- is enforced at the application layer.
--
-- Optimistic-locking via `version` follows the pattern established for
-- flags / segments / event_definitions.

CREATE TABLE metric_definitions (
    id              UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    environment_id  UUID        NOT NULL REFERENCES environments(id),
    key             TEXT        NOT NULL,
    name            TEXT        NOT NULL,
    description     TEXT,
    kind            TEXT        NOT NULL CHECK (kind IN ('aggregation', 'ratio', 'funnel')),
    config          JSONB       NOT NULL,
    goal_direction  TEXT        NOT NULL DEFAULT 'increase'
                                CHECK (goal_direction IN ('increase', 'decrease', 'neutral')),
    version         BIGINT      NOT NULL DEFAULT 1,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at      TIMESTAMPTZ
);

-- Live-row uniqueness on (env_id, key): partial unique constraint so we
-- can soft-delete a metric and re-create another with the same key.
CREATE UNIQUE INDEX idx_metric_definitions_env_key_live
    ON metric_definitions(environment_id, key)
    WHERE deleted_at IS NULL;

-- Fast env-scoped listing (the most common query path in the API).
CREATE INDEX idx_metric_definitions_environment_id_live
    ON metric_definitions(environment_id)
    WHERE deleted_at IS NULL;

-- Kind-filtered listing — used by the admin UI's kind tabs (aggregations
-- vs ratios vs funnels) and by the experiment-creation metric picker
-- which filters by kind.
CREATE INDEX idx_metric_definitions_kind
    ON metric_definitions(environment_id, kind)
    WHERE deleted_at IS NULL;
