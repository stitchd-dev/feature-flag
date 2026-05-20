-- Migration 20260520000003: experiment_iterations.metric_keys → metric_ids
--
-- Follow-up to 20260520000002_experiment_metrics_cutover, which converted
-- `experiments.metric_keys TEXT[]` → `metric_ids UUID[]`. The iterations table
-- is the snapshot taken when an experiment transitions into `running` — it
-- has to stay aligned with the experiments table's column type, otherwise
-- the Rust repo can't snapshot a `Vec<MetricId>` into a `TEXT[]` column
-- without an awkward stringification.
--
-- Pre-launch: no live iterations exist (verified — table is empty), so this
-- is a clean drop-and-add. CASCADE is not needed: no view / index / FK
-- references this column.
--
-- Idempotent: re-running is a no-op once `metric_ids` exists.

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'experiment_iterations' AND column_name = 'metric_ids'
    ) THEN
        RAISE NOTICE 'experiment_iterations_metric_ids already applied; skipping';
        RETURN;
    END IF;

    ALTER TABLE experiment_iterations DROP COLUMN metric_keys;
    ALTER TABLE experiment_iterations ADD COLUMN metric_ids UUID[] NOT NULL DEFAULT '{}';
END $$;
