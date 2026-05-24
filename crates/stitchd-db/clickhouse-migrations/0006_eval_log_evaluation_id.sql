-- Migration 0006: add evaluation_id to flag_evaluation_log and flag_evaluation_log_v2.
--
-- Background: BUG-030 — flag evaluations against a multi-context bundle
-- (e.g. user + device) emit one row per context type, but no identifier
-- links sibling rows from the same evaluation call. experiment_assignments_mv
-- cannot correctly attribute cross-context bundle membership without this.
--
-- evaluation_id is stamped by eval_log_writer::build_eval_log_rows: one UUID
-- per EvaluationContext, shared by all context rows in that bundle.
-- DEFAULT generateUUIDv4() provides a valid fallback for rows written by older
-- code before the writer was updated (pre-migration rows get a random UUID,
-- which is harmless — they simply cannot be grouped with siblings).

ALTER TABLE flag_evaluation_log
    ADD COLUMN IF NOT EXISTS evaluation_id UUID DEFAULT generateUUIDv4();

ALTER TABLE flag_evaluation_log_v2
    ADD COLUMN IF NOT EXISTS evaluation_id UUID DEFAULT generateUUIDv4();
