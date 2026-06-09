-- Add evaluation_id to flag_evaluation_log (BUG-030 / schema-cutover gap).
--
-- The schema_cutover_20260525 v1 baseline OMITTED the evaluation_id column that
-- legacy migration 0006 had added — but EvalLogRow, and the SDK eval-log ingest
-- path (flag-service sdk_backend::event_to_row -> insert_eval_log_rows), WRITE
-- evaluation_id. Without this column, IngestSdkEvalLog fails with SchemaMismatch
-- on any fresh deployment (the canonical migrator, event_writer::migrations::run,
-- never created it). This forward migration restores it for both fresh and
-- already-migrated deployments.
--
-- evaluation_id stamps one UUID per multi-context evaluation bundle so the
-- per-context-type sibling rows can be grouped (experiment_assignments_mv
-- cross-context attribution). DEFAULT generateUUIDv4() gives any pre-column rows
-- a harmless random id. Type is non-nullable UUID to match
-- EvalLogRow.evaluation_id (`serde(with = "clickhouse::serde::uuid")`, not
-- ::option). Idempotent via IF NOT EXISTS; flag_evaluation_log is a plain
-- MergeTree so no ON CLUSTER is required. The MV reads explicit columns and does
-- not reference evaluation_id, so it is unaffected.

ALTER TABLE flag_evaluation_log
    ADD COLUMN IF NOT EXISTS evaluation_id UUID DEFAULT generateUUIDv4();
