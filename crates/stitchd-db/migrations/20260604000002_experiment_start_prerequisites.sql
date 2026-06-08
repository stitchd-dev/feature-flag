-- =============================================================================
-- Experiment start-time prerequisites
-- (flag_lifecycle_20260604, Phase 5 Task 2). Purely additive.
--
-- An experiment may declare start-time prerequisites that must hold for it to be
-- started (manual OR scheduled — both go through TransitionExperiment's start
-- path). Two kinds:
--   * 'flag_variant'    — the named flag must currently serve a required variant
--                         (prerequisite_flag_id + required_variant_id set)
--   * 'experiment_done' — the referenced experiment must be stopped/concluded
--                         (prerequisite_experiment_id set)
-- If any prerequisite is unmet at start time, the start is rejected with a clear
-- reason (FAILED_PRECONDITION; surfaced as 409 on a gateway path).
-- =============================================================================

CREATE TABLE IF NOT EXISTS experiment_start_prerequisites (
    id                        UUID NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    -- The experiment whose start is gated by this prerequisite.
    experiment_id             UUID NOT NULL REFERENCES experiments(id) ON DELETE CASCADE,
    -- Discriminator for the prerequisite shape.
    kind                      TEXT NOT NULL
                              CONSTRAINT chk_experiment_start_prereq_kind
                              CHECK (kind IN ('flag_variant', 'experiment_done')),
    -- 'flag_variant': the flag that must serve `required_variant_id`.
    prerequisite_flag_id      UUID,
    -- 'flag_variant': the variant the prerequisite flag must currently serve.
    required_variant_id       UUID,
    -- 'experiment_done': the experiment that must be stopped/concluded.
    prerequisite_experiment_id UUID,
    -- Stable ordering for display / deterministic evaluation.
    "order"                   INT  NOT NULL DEFAULT 0,
    created_at                TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Shape integrity: each kind populates exactly its own columns.
    CONSTRAINT chk_experiment_start_prereq_shape CHECK (
        (kind = 'flag_variant'
            AND prerequisite_flag_id IS NOT NULL
            AND required_variant_id IS NOT NULL
            AND prerequisite_experiment_id IS NULL)
        OR
        (kind = 'experiment_done'
            AND prerequisite_experiment_id IS NOT NULL
            AND prerequisite_flag_id IS NULL
            AND required_variant_id IS NULL)
    )
);

-- "prerequisites for this experiment" — the start-time gate lookup.
CREATE INDEX IF NOT EXISTS idx_experiment_start_prereq_experiment
    ON experiment_start_prerequisites (experiment_id);
