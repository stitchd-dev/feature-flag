-- Migration 20260520000004: add admin-UI columns to event_definitions
--
-- The pre-existing event_definitions schema only had (key, value_type) —
-- enough for ingestion-time type enforcement but missing the admin
-- classification + display fields the events_metrics_20260519 admin UI
-- assumes (`name`, `description`, `metric_type`, `schema`).
--
-- After this migration the table carries BOTH:
--   * `value_type` (bool|int|double) — type-checked at the ingestion path
--     (analytics-service's TrackEvents RPC)
--   * `metric_type` (count|conversion|revenue|duration|numeric|custom) — the
--     admin classification surfaced in the UI + used by the experiment
--     metric picker
--
-- `name` defaults to the key when null (the admin UI renders `name ?? key`
-- already). `description` and `schema` are optional.

ALTER TABLE event_definitions
    ADD COLUMN IF NOT EXISTS name        TEXT,
    ADD COLUMN IF NOT EXISTS description TEXT,
    ADD COLUMN IF NOT EXISTS metric_type TEXT NOT NULL DEFAULT 'count'
        CHECK (metric_type IN ('count', 'conversion', 'revenue', 'duration', 'numeric', 'custom')),
    ADD COLUMN IF NOT EXISTS schema      JSONB;

-- Backfill `name` from `key` for existing rows so the UI doesn't render
-- empty cells. Idempotent — only touches rows where name is still null.
UPDATE event_definitions
SET name = key
WHERE name IS NULL;

-- Optional fast-list index on (environment_id, metric_type) for the admin
-- UI's kind-filter dropdown. Partial — only live rows.
CREATE INDEX IF NOT EXISTS idx_event_definitions_env_metric_type_live
    ON event_definitions(environment_id, metric_type)
    WHERE deleted_at IS NULL;
