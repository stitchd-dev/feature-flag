-- 0004_segment_list_summary: Counter rows for get_list_segment_summary.
--
-- Maintains running include_count and exclude_count per (segment_id, context_type)
-- so the admin UI can display counts without scanning the full entries table.
-- Counters are updated atomically with each add_entries / remove_entries call.
CREATE TABLE IF NOT EXISTS __KEYSPACE__.segment_list_summary (
    segment_id    UUID,
    context_type  TEXT,
    include_count COUNTER,
    exclude_count COUNTER,
    PRIMARY KEY (segment_id, context_type)
);
