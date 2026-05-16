-- FR-6: Single-column last_seen_at indexes for context registry purge queries.
--
-- The 90-day purge runs:
--   DELETE FROM context_type_registry  WHERE last_seen_at < $1
--   DELETE FROM context_param_registry WHERE last_seen_at < $1
--
-- Without these indexes, PostgreSQL performs a full table scan on both tables
-- for every purge cycle. B-tree indexes on last_seen_at let the planner use
-- an index scan and locate only qualifying rows.
--
-- Production note: for zero-downtime deployment use CREATE INDEX CONCURRENTLY.
CREATE INDEX IF NOT EXISTS idx_context_type_registry_last_seen
    ON context_type_registry(last_seen_at);

CREATE INDEX IF NOT EXISTS idx_context_param_registry_last_seen
    ON context_param_registry(last_seen_at);
