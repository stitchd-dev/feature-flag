-- 0002_segment_list_entries: Main entries table for list-based segments.
--
-- Partition key : (segment_id, context_type, generation)
-- Clustering key: (list_type, entry_key)
--
-- Each full-replace operation writes to a new generation; the active generation
-- is stored in the pointer table (migration 0003). This ensures readers always
-- see a consistent snapshot.
CREATE TABLE IF NOT EXISTS __KEYSPACE__.segment_list_entries (
    segment_id   UUID,
    context_type TEXT,
    generation   BIGINT,
    list_type    TEXT,   -- 'include' | 'exclude'
    entry_key    TEXT,
    PRIMARY KEY ((segment_id, context_type, generation), list_type, entry_key)
) WITH clustering order by (list_type ASC, entry_key ASC)
  AND gc_grace_seconds = 86400;
