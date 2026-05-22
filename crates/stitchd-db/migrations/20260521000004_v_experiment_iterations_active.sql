-- Migration 20260521000004: v_experiment_iterations_active view + audit table
--
-- Backing source for the ClickHouse `experiment_iterations_active` DICTIONARY
-- (CH migration 20260521000002). Keeps the SQL the dictionary needs in one
-- place so we can evolve it (or swap the join shape) without re-creating the
-- dictionary on every refresh.
--
-- Row shape: one row per `(env_id, flag_id, matched_rule_id, context_type)`
-- tuple that is currently in scope for attribution. `context_type` is
-- cross-product-expanded from `unit_context_types` via `UNNEST(...)`.
--
-- Matching semantics for the MV's `dictGet`:
--
--   * A rule-bound iteration emits `matched_rule_id = experiments.flag_rule_id`.
--   * A default-rule-bound iteration emits `matched_rule_id = NULL`. The CH
--     dictionary stores that as `Nullable(UUID)` with `COMPLEX_KEY_HASHED()`,
--     which matches a NULL key part against a NULL eval-log column directly.
--     If that path turns out to be unstable on CH 24.10.2 (the running
--     server), we'll fall back to the sentinel `00000000-...` UUID in a
--     follow-up — documented in the dictionary migration.
--
-- Status filter: `running` OR `paused`. Paused iterations are still in scope
-- — first-exposure semantics mean any eval against a paused experiment still
-- counts as an exposure for the iteration's analysis (the iteration is just
-- not currently allocating new traffic).
--
-- ALSO creates `experiment_iterations_active_audit (updated_at)` — a
-- single-row table whose `updated_at` is bumped by an AFTER INSERT/UPDATE
-- trigger on `experiment_iterations` + `experiments`. CH's dictionary
-- `invalidate_query` polls `SELECT max(updated_at)` against this table to
-- decide whether the LIFETIME refresh actually needs to re-pull rows.
--
-- Idempotent — guarded on view existence.

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.views
        WHERE table_schema = 'public'
          AND table_name = 'v_experiment_iterations_active'
    ) THEN
        RAISE NOTICE 'v_experiment_iterations_active already exists; skipping';
        RETURN;
    END IF;

    -- Audit table for dictionary invalidate_query polling.
    -- Single-row design: there is exactly one row with id=1; the trigger
    -- bumps `updated_at = now()` on every relevant DML.
    CREATE TABLE IF NOT EXISTS experiment_iterations_active_audit (
        id         INT PRIMARY KEY CHECK (id = 1),
        updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
    );

    INSERT INTO experiment_iterations_active_audit (id, updated_at)
        VALUES (1, now())
        ON CONFLICT (id) DO NOTHING;

    -- View: one row per (env_id, flag_id, matched_rule_id, context_type) for
    -- every iteration whose parent experiment is `running` or `paused`.
    -- Active iteration = the OPEN iteration (`ended_at IS NULL`) of a
    -- not-deleted experiment in running/paused status.
    CREATE VIEW v_experiment_iterations_active AS
    SELECT
        e.env_id                                  AS env_id,
        ei.flag_id                                AS flag_id,
        e.flag_rule_id                            AS matched_rule_id,
        ct                                        AS context_type,
        ei.id                                     AS iteration_id,
        ei.experiment_id                          AS experiment_id,
        ei.iteration_number                       AS iteration_number,
        ei.started_at                             AS started_at,
        ei.ended_at                               AS ended_at
    FROM experiment_iterations ei
    JOIN experiments e ON e.id = ei.experiment_id
    CROSS JOIN LATERAL UNNEST(ei.unit_context_types) AS ct
    WHERE e.status IN ('running', 'paused')
      AND e.deleted_at IS NULL
      AND ei.ended_at IS NULL;

    -- Trigger function to bump the audit row.
    CREATE OR REPLACE FUNCTION bump_experiment_iterations_active_audit()
    RETURNS TRIGGER AS $f$
    BEGIN
        UPDATE experiment_iterations_active_audit
           SET updated_at = now()
         WHERE id = 1;
        RETURN NULL;
    END;
    $f$ LANGUAGE plpgsql;

    CREATE TRIGGER trg_experiments_bump_iter_active_audit
    AFTER INSERT OR UPDATE OR DELETE ON experiments
    FOR EACH ROW
    EXECUTE FUNCTION bump_experiment_iterations_active_audit();

    CREATE TRIGGER trg_iterations_bump_iter_active_audit
    AFTER INSERT OR UPDATE OR DELETE ON experiment_iterations
    FOR EACH ROW
    EXECUTE FUNCTION bump_experiment_iterations_active_audit();
END $$;
