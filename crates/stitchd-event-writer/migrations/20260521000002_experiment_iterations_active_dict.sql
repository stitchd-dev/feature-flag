-- ClickHouse Migration 20260521000002: experiment_iterations_active dictionary
--
-- Backs the `experiment_assignments_mv` materialized view's `dictGet` lookups.
-- One dictionary entry per
--   (env_id, flag_id, matched_rule_id, context_type)
-- tuple that corresponds to a currently-open iteration of a running-or-paused
-- experiment. The MV uses `dictHas(...)` as a WHERE filter (scope an eval-log
-- row to an in-progress experiment) and `dictGet(..., 'experiment_id', ...)`
-- to pull the experiment_id into the MV row.
--
-- Source: PG view `public.v_experiment_iterations_active` (created in PG
-- migration 20260521000004). The view does the JOIN + UNNEST so this DDL
-- stays trivial and the underlying SQL can evolve without re-issuing the
-- dictionary's CREATE.
--
-- ## Nullable key part — `matched_rule_id`
--
-- CH `COMPLEX_KEY_HASHED()` supports `Nullable(UUID)` key parts on
-- ClickHouse 24.10.x — verified against the running container before
-- committing this migration. A NULL key part matches a NULL `matched_rule_id`
-- column from `flag_evaluation_log` directly, which is exactly the
-- "default-rule-bound experiment + flag fell through to default rule" case.
-- If a later CH upgrade regresses this behaviour, fall back to encoding
-- the default-rule case as the sentinel UUID `00000000-...` in both the PG
-- view and the eval-log writer.
--
-- ## Network reachability
--
-- The dev stack has both PG and CH on the `stitchd` Docker network, so both
-- `stitchd-postgres` (container-DNS) and `host.docker.internal`
-- (Docker-host-bridge) resolve from inside CH. We use `host.docker.internal`
-- to match the convention already established for production deploys where
-- PG runs outside the CH container's network namespace. Verified resolvable
-- via `getent hosts host.docker.internal` inside `stitchd-clickhouse`.
--
-- ## Refresh
--
-- LIFETIME(MIN 30 MAX 60) — dictionary refreshes every 30-60 seconds.
-- `invalidate_query` polls the
-- `public.experiment_iterations_active_audit.updated_at` column (bumped by
-- triggers on `experiments` + `experiment_iterations` DML) so the refresh
-- only re-pulls rows when the view's contents could have changed. For
-- immediate updates on transition the
-- `stitchd-experimentation-service::apply_transition` path issues
-- `SYSTEM RELOAD DICTIONARY experiment_iterations_active` against CH; the
-- 30s TTL bounds the staleness window even if that fire-and-forget call
-- fails.

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
    invalidate_query 'SELECT max(updated_at) FROM public.experiment_iterations_active_audit'
))
LIFETIME(MIN 30 MAX 60)
LAYOUT(COMPLEX_KEY_HASHED());
