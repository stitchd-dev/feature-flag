-- =============================================================================
-- V1 Baseline — stitchd ScyllaDB schema (post-cutover 2026-05-25)
-- Synthesised from CQL migrations 0001 through 0005.
-- =============================================================================

-- Keyspace: replication configured by the bootstrap applier at deploy time.
CREATE KEYSPACE IF NOT EXISTS stitchd_segments
    WITH replication = {'class': 'NetworkTopologyStrategy', 'replication_factor': 3}
    AND durable_writes = true;

-- Main entries table; each full-replace writes to a new generation partition for atomic swap.
CREATE TABLE IF NOT EXISTS stitchd_segments.segment_list_entries (
    segment_id   UUID,
    context_type TEXT,
    generation   BIGINT,
    list_type    TEXT,
    entry_key    TEXT,
    PRIMARY KEY ((segment_id, context_type, generation), list_type, entry_key)
) WITH clustering order by (list_type ASC, entry_key ASC)
  AND gc_grace_seconds = 86400;

-- Pointer table for atomic generation swap; one row per (segment_id, context_type).
CREATE TABLE IF NOT EXISTS stitchd_segments.segment_list_generations (
    segment_id        UUID,
    context_type      TEXT,
    active_generation BIGINT,
    updated_at        TIMESTAMP,
    PRIMARY KEY (segment_id, context_type)
);

-- Counter rows for get_list_segment_summary; running include/exclude counts.
CREATE TABLE IF NOT EXISTS stitchd_segments.segment_list_summary (
    segment_id    UUID,
    context_type  TEXT,
    include_count COUNTER,
    exclude_count COUNTER,
    PRIMARY KEY (segment_id, context_type)
);

-- Orphaned-generation tracker for the sweeper; records superseded generations pending cleanup.
CREATE TABLE IF NOT EXISTS stitchd_segments.segment_list_orphaned_gens (
    segment_id   UUID,
    context_type TEXT,
    generation   BIGINT,
    orphaned_at  TIMESTAMP,
    PRIMARY KEY (segment_id, context_type, generation)
);
