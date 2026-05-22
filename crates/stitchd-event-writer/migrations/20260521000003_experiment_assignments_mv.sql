-- ClickHouse Migration 20260521000003: experiment_assignments table + MV
--
-- Phase 4 Task 2 — derive first-exposure experiment assignments from
-- `flag_evaluation_log` and persist them in `experiment_assignments`.
--
-- ## Table shape
--
-- Per the experimentation_full_20260521 spec (§1 Attribution Pipeline), the
-- assignment row carries:
--   * experiment_id, iteration_id    — identifies the analysis bucket
--   * flag_id, env_id                — denormalised for hot-path filters
--   * context_type, context_key      — the assignment unit
--   * variant_key                    — the cohort the unit landed in
--   * assigned_at                    — the first-exposure timestamp (the
--                                      MV's argMin-like behaviour)
--   * matched_rule_id Nullable(UUID) — copied from the eval row for traceability
--
-- The old `clickhouse-migrations/0002_experiment_assignments.sql` reference
-- doc (not registered in the live runner) used a sparser schema keyed only on
-- `(experiment_id, context_key)` with `ReplacingMergeTree(assigned_at)`. We
-- recreate the table fresh here under the modern shape — pre-launch there are
-- no rows to migrate.
--
-- ## First-exposure semantics on ReplacingMergeTree
--
-- ReplacingMergeTree(<version>) keeps the row with the MAX `<version>` during
-- merges. We want FIRST-exposure (the MIN timestamp). Trick: store
-- `-toUnixTimestamp64Milli(evaluated_at)` as a signed `_version` column —
-- the MAX of the negated values is the MIN of the original timestamps.
-- `assigned_at` itself stays in human-friendly form for queries; the
-- `_version` column is the merge tiebreaker.
--
-- This relies on:
--   * INSERT-time eager merge being non-load-bearing (we tolerate brief windows
--     where multiple unmerged rows exist for the same key — readers MUST
--     `FINAL` or `argMin(...)` to collapse).
--   * Stats queries in Phase 5 will SELECT with `argMin(variant_key, assigned_at)`
--     anyway, so the merge collapsing is purely a storage-side compaction.
--
-- ## Order key
--
-- `(experiment_id, iteration_id, context_type, context_key)` — matches the
-- spec's per-iteration, per-context-type dedup unit. `env_id` and `flag_id`
-- are denormalised payload, not part of the key.
--
-- ## MV body
--
-- For every eval-log row with `targeting_on = true` AND
-- `dictHas('experiment_iterations_active', (env_id, flag_id, matched_rule_id, context_type))`:
--   * Emit one row to `experiment_assignments` with the iteration's
--     experiment_id + iteration_id resolved via `dictGet`.
--   * `_version = -toUnixTimestamp64Milli(evaluated_at)` so merges keep the
--     earliest eval per dedup key.
--
-- The dictionary's `Nullable(UUID)` `matched_rule_id` matches the eval row's
-- own `Nullable(UUID)` `matched_rule_id` directly — NULL ↔ NULL for
-- default-rule-bound experiments, UUID ↔ UUID for rule-bound.

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

CREATE MATERIALIZED VIEW IF NOT EXISTS experiment_assignments_mv
TO experiment_assignments AS
SELECT
    dictGet('experiment_iterations_active', 'experiment_id',
            (e.env_id, e.flag_id, e.matched_rule_id, e.context_type))   AS experiment_id,
    dictGet('experiment_iterations_active', 'iteration_id',
            (e.env_id, e.flag_id, e.matched_rule_id, e.context_type))   AS iteration_id,
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
