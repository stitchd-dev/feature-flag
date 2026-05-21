-- ClickHouse Migration 20260521000004: backfill experiment_assignments
--
-- Phase 4 Task 3 — populate experiment_assignments from the last 90 days
-- of flag_evaluation_log history. Mirrors the MV body so live + historical
-- attribution stay consistent.
--
-- ## One-shot semantics
--
-- This migration runs once per fresh CH (the `_schema_migrations` table
-- prevents re-runs). If a new experiment iteration starts AFTER this
-- migration applies, attribution catches it via the MV's go-forward path
-- — no further backfill needed unless a long backfill window opens up.
--
-- ## Manual re-backfill
--
-- For a manual incremental backfill outside the migration system (e.g.
-- when a new experiment binds a flag that already had evaluation history),
-- run:
--
--   INSERT INTO experiment_assignments
--   SELECT ... -- same SELECT as below
--   FROM flag_evaluation_log AS e
--   WHERE e.targeting_on = true
--     AND e.evaluated_at >= now() - INTERVAL 90 DAY
--     AND e.matched_rule_id IS NOT NULL  -- skip pre-migration rows
--     AND dictHas('experiment_iterations_active',
--                 (e.env_id, e.flag_id, e.matched_rule_id, e.context_type));
--
-- ## Pre-migration row filter
--
-- `flag_evaluation_log` rows written before migration
-- `20260521000001_flag_eval_log_matched_rule` lack a `matched_rule_id`
-- value (it defaulted to NULL on the existing rows). Those NULLs would
-- conflate with default-rule-bound experiments — we'd backfill a single
-- pre-migration eval as an assignment for an unrelated default-rule
-- experiment if one happens to bind the same (env_id, flag_id, context_type)
-- tuple.
--
-- The spec calls this out explicitly: "Backfill is skipped for rows where
-- matched_rule_id is absent (pre-migration rows) — those contexts get
-- attributed only from go-forward evals."
--
-- Discriminating "pre-migration NULL" from "default-rule fallthrough NULL"
-- is not possible from `flag_evaluation_log` alone (both look identical at
-- rest). The safe choice is to require `matched_rule_id IS NOT NULL` in
-- the backfill — we lose default-rule-bound attributions for the 90-day
-- window, but the MV catches them going forward. Eval-log rows written
-- AFTER `20260521000001` for default-rule-bound experiments will have
-- matched_rule_id = NULL but ALSO carry the right targeting_on=true
-- semantics; they're simply not covered by this one-shot backfill.
--
-- (Trade-off documented; deviation from a pure mirror of the MV is
-- intentional and matches the spec's "go-forward only for matched_rule_id
-- IS NULL during the 90-day window" stance.)
--
-- ## Bound
--
-- `evaluated_at >= now() - INTERVAL 90 DAY` matches the
-- flag_evaluation_log TTL — we never look further back than CH retains.

INSERT INTO experiment_assignments
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
  AND e.evaluated_at >= now() - INTERVAL 90 DAY
  AND e.matched_rule_id IS NOT NULL
  AND dictHas('experiment_iterations_active',
              (e.env_id, e.flag_id, e.matched_rule_id, e.context_type));
