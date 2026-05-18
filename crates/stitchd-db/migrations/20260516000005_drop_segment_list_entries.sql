-- 20260516000005: Drop segment_list_entries table and associated indexes.
--
-- List-based segment membership is now stored in ScyllaDB (see scylla-migrations/).
-- PostgreSQL is retained for segment metadata only.

DROP INDEX IF EXISTS idx_segment_list_entries_covering;
DROP INDEX IF EXISTS idx_segment_list_entries_lookup;
DROP TABLE IF EXISTS segment_list_entries CASCADE;
