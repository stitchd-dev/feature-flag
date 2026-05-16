-- FR-4: Covering index for segment list entry EXISTS lookups.
--
-- The membership-check subquery pattern is:
--   EXISTS (SELECT 1 FROM segment_list_entries
--           WHERE segment_id = $1 AND context_type = $2
--             AND list_type = 'include' AND entry_key = $3)
--
-- The old index (segment_id, context_type, list_type) covered 3 of the 4
-- predicate columns, requiring a heap fetch to resolve entry_key. Adding
-- entry_key as the 4th column makes this an index-only scan.
--
-- segment_list_entries is a partitioned table; the index on the parent is
-- propagated to all existing and future child partitions automatically.
--
-- Production note: for zero-downtime deployment, run each step individually
-- in autocommit mode using DROP INDEX CONCURRENTLY / CREATE INDEX CONCURRENTLY.
DROP INDEX IF EXISTS idx_segment_list_entries_lookup;

CREATE INDEX IF NOT EXISTS idx_segment_list_entries_covering
    ON segment_list_entries(segment_id, context_type, list_type, entry_key);
