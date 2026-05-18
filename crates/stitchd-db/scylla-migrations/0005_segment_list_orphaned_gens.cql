-- 0005_segment_list_orphaned_gens: Tracks superseded generations for the sweeper.
--
-- When set_list_entries successfully CAS-flips the generation pointer, the
-- previous generation becomes unreachable but its partition remains in
-- segment_list_entries.  The sweeper reads this table to find generations that
-- are safe to delete (orphaned longer than the retention window) and removes
-- the corresponding partition from segment_list_entries.
--
-- Rows are deleted from this table after the sweeper successfully cleans up
-- the corresponding segment_list_entries partition.
CREATE TABLE IF NOT EXISTS __KEYSPACE__.segment_list_orphaned_gens (
    segment_id   UUID,
    context_type TEXT,
    generation   BIGINT,
    orphaned_at  TIMESTAMP,
    PRIMARY KEY (segment_id, context_type, generation)
);
