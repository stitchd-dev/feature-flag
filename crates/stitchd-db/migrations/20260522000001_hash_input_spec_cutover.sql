-- Migration 20260522000001: hash_input_spec_cutover
--
-- Phase 3 of `flag_eval_unify_20260522` — schema cutover preparing the unified
-- `HashInputSpec` percentage-hash schema introduced in `stitchd-core` Phase 1.
--
-- ── Dual-schema state ────────────────────────────────────────────────────────
-- This migration deliberately adds the NEW columns alongside the EXISTING
-- payloads. The legacy in-row JSON (`feature_flag_rules.rule_def` with
-- `output.Percentage.targets`, and `feature_flags.default_rule_distribution`)
-- is NOT dropped here. The repo layer + service layer continue to read both;
-- Phase 5/6 of the same track drops the legacy fields after every caller
-- (gateway DTOs, gRPC mapping, SDK conformance fixtures) has migrated.
--
-- Why dual schema? The track is split across parallel workers — schema/proto
-- (P3, this commit) lands before the repo/service rewires (P2 + Phase 5/6).
-- A clean cutover that drops `rule_def` here would break the repo layer on
-- merge. Dual-schema keeps the worktrees independent.
--
-- ── What this migration adds ─────────────────────────────────────────────────
-- 1. `feature_flag_rules.hash_inputs jsonb NULL` — the canonical ordered
--    selector list `[{kind, context_type [, parameter]}...]` for any rule
--    whose existing `rule_def->>'output'` carries a `Percentage` allocation.
--    NULL means "this rule is a Variant output, or rules without percentage
--    allocation". Variant rules don't carry hash inputs.
--
-- 2. `feature_flags.default_rule_hash_inputs jsonb NULL` — companion column
--    next to `default_rule_distribution`. NULL means "no default-rule
--    distribution configured" (the current `default_rule_distribution IS NULL`
--    case). Today the preview path hashes the primary context's key without an
--    explicit per-flag hash-input config (see
--    `stitchd-core::evaluation::preview::evaluate_single`); the new column
--    makes that implicit choice author-controllable.
--
-- ── Backfill semantics ───────────────────────────────────────────────────────
-- For each rule with a Percentage output in `rule_def`, the existing
-- `targets: Vec<PercentageTarget>` is already an ordered list — but the
-- ordering reflects whatever upstream code path produced it (insertion order
-- of the proto `context_hash_specs` HashMap was non-deterministic in pre-1.0
-- callers). The backfill emits an ordered selector list under the canonical
-- sort: `context_type ASC, parameter ASC within type`.
--
-- For rules where the legacy ordering already matched canonical sort, the
-- hash bucket assigned by `calculate_allocation` is unchanged (NFR-4 in the
-- track spec). For rules where the legacy ordering differed, the bucket
-- shifts — this is bucket-reshuffling that requires operator sign-off. The
-- `verify-hash-cutover` xtask binary (committed in this phase) enumerates
-- the affected rules.
--
-- For the `default_rule_distribution` path, no per-flag hash-input config
-- exists today, so the backfill leaves `default_rule_hash_inputs` NULL on
-- existing rows. Operators populate it via the admin UI in Phase 7.
--
-- ── Idempotency ──────────────────────────────────────────────────────────────
-- Standard `ADD COLUMN IF NOT EXISTS` (the project's `feature_flags`
-- precedent). The backfill query uses an `UPDATE … WHERE hash_inputs IS NULL`
-- guard so re-running the migration is a no-op.

-- Step 1: Add the new columns.
ALTER TABLE feature_flag_rules
    ADD COLUMN IF NOT EXISTS hash_inputs JSONB NULL;

ALTER TABLE feature_flags
    ADD COLUMN IF NOT EXISTS default_rule_hash_inputs JSONB NULL;

-- Step 2: Backfill `feature_flag_rules.hash_inputs` from
-- `rule_def->'output'->'Percentage'->'targets'`.
--
-- The legacy `targets` array contains objects of shape:
--   { "context_type": "<ct>", "field": "Key" }            ← key-based selector
--   { "context_type": "<ct>", "field": { "Parameter": "<p>" } }  ← parameter selector
--
-- Emits new shape:
--   { "kind": "context_key",       "context_type": "<ct>" }
--   { "kind": "context_parameter", "context_type": "<ct>", "parameter": "<p>" }
--
-- The CTE flattens, then re-aggregates in canonical sort order
-- (`context_type ASC, parameter ASC within type`).
WITH expanded AS (
    SELECT
        r.id                                      AS rule_id,
        (t.value->>'context_type')                AS context_type,
        CASE
            WHEN t.value->'field' = '"Key"'::jsonb THEN NULL
            ELSE t.value->'field'->>'Parameter'
        END                                       AS parameter,
        CASE
            WHEN t.value->'field' = '"Key"'::jsonb THEN 'context_key'
            ELSE 'context_parameter'
        END                                       AS kind
    FROM feature_flag_rules r,
         LATERAL jsonb_array_elements(
             COALESCE(r.rule_def->'output'->'Percentage'->'targets', '[]'::jsonb)
         ) AS t(value)
    WHERE r.hash_inputs IS NULL
),
aggregated AS (
    SELECT
        rule_id,
        jsonb_agg(
            CASE
                WHEN parameter IS NULL THEN
                    jsonb_build_object(
                        'kind',         'context_key',
                        'context_type', context_type
                    )
                ELSE
                    jsonb_build_object(
                        'kind',         'context_parameter',
                        'context_type', context_type,
                        'parameter',    parameter
                    )
            END
            ORDER BY context_type ASC, COALESCE(parameter, '') ASC
        ) AS hash_inputs
    FROM expanded
    GROUP BY rule_id
)
UPDATE feature_flag_rules r
   SET hash_inputs = a.hash_inputs
  FROM aggregated a
 WHERE r.id = a.rule_id
   AND r.hash_inputs IS NULL;

-- Step 3: `feature_flags.default_rule_hash_inputs` backfill is intentionally
-- empty — see header comment. Operators populate via Phase 7 admin UI.
