-- 0003_segment_list_generations: Pointer table for atomic generation swap.
--
-- Each (segment_id, context_type) pair has exactly one row here naming the
-- current active generation. Full-replace uses a Lightweight Transaction (LWT)
-- to flip active_generation atomically after writing all entries for new_gen.
CREATE TABLE IF NOT EXISTS __KEYSPACE__.segment_list_generations (
    segment_id        UUID,
    context_type      TEXT,
    active_generation BIGINT,
    updated_at        TIMESTAMP,
    PRIMARY KEY (segment_id, context_type)
);
