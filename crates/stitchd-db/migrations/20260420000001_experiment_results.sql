-- Migration 20260420000001: experiment_results
--
-- Adds statistical analysis support to the Experimentation Module:
--   - analysis_type column on experiments (frequentist | bayesian, default frequentist)
--   - experiment_results: computed analysis results per metric per iteration

-- ---------------------------------------------------------------------------
-- experiments: add analysis_type column
-- ---------------------------------------------------------------------------
ALTER TABLE experiments
    ADD COLUMN analysis_type TEXT NOT NULL DEFAULT 'frequentist'
        CHECK (analysis_type IN ('frequentist', 'bayesian'));

-- ---------------------------------------------------------------------------
-- experiment_results
-- ---------------------------------------------------------------------------
-- Stores computed statistical results for each metric in an experiment
-- iteration.  One row per (experiment, iteration, metric) triple.
-- variant_stats, frequentist_result, and bayesian_result are JSONB so that
-- the shape can evolve without further schema migrations.
CREATE TABLE experiment_results (
    id                   UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    experiment_id        UUID        NOT NULL REFERENCES experiments(id),
    iteration_id         UUID        NOT NULL REFERENCES experiment_iterations(id),
    metric_key           TEXT        NOT NULL,
    metric_type          TEXT        NOT NULL
                             CHECK (metric_type IN ('count', 'numeric', 'percentile', 'funnel')),
    variant_stats        JSONB       NOT NULL,
    frequentist_result   JSONB,
    bayesian_result      JSONB,
    recommendation       TEXT        NOT NULL,
    computed_at          TIMESTAMPTZ NOT NULL,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (experiment_id, iteration_id, metric_key)
);

CREATE INDEX idx_experiment_results_experiment_iteration
    ON experiment_results (experiment_id, iteration_id);
