-- Migration 20260423000001: stats_tables
--
-- Adds job tracking and schedule state tables for the stats microservice.
-- The stats service runs experiment result computations on a 60-minute
-- schedule rather than on-demand; these tables track that lifecycle.

-- ---------------------------------------------------------------------------
-- stats_jobs
-- ---------------------------------------------------------------------------
-- One row per computation job. The stats service creates a row (pending)
-- on each scheduler tick or manual /recompute request, then transitions it
-- through running → completed | failed.
CREATE TABLE stats_jobs (
    id              UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    experiment_id   UUID,       -- nullable: future support for non-experiment jobs
    status          TEXT        NOT NULL DEFAULT 'pending'
                        CHECK (status IN ('pending', 'running', 'completed', 'failed')),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at      TIMESTAMPTZ,
    completed_at    TIMESTAMPTZ,
    error           TEXT
);

CREATE INDEX idx_stats_jobs_experiment_id ON stats_jobs (experiment_id)
    WHERE experiment_id IS NOT NULL;
CREATE INDEX idx_stats_jobs_status ON stats_jobs (status);

-- ---------------------------------------------------------------------------
-- stats_schedule
-- ---------------------------------------------------------------------------
-- One row per experiment. Tracks when stats were last computed, when the
-- next run is scheduled, and the current computation status. Upserted by
-- the stats service after each successful or failed computation.
CREATE TABLE stats_schedule (
    experiment_id       UUID        NOT NULL REFERENCES experiments(id) PRIMARY KEY,
    last_computed_at    TIMESTAMPTZ,
    next_run_at         TIMESTAMPTZ,
    computation_status  TEXT        NOT NULL DEFAULT 'never_computed'
                            CHECK (computation_status IN ('ready', 'computing', 'never_computed')),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);
