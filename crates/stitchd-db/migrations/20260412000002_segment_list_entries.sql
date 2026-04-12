-- Migration 002_20260412: segment_list_entries
--
-- List-based segments store their included/excluded keys here.
-- Monthly range-partitioned on created_at via pg_partman (if available).

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_available_extensions WHERE name = 'pg_partman') THEN
        CREATE EXTENSION IF NOT EXISTS pg_partman;
    END IF;
END $$;

CREATE TABLE segment_list_entries (
  id           UUID        NOT NULL DEFAULT gen_random_uuid(),
  segment_id   UUID        NOT NULL REFERENCES segments(id) ON DELETE CASCADE,
  context_type TEXT        NOT NULL,
  entry_key    TEXT        NOT NULL,
  list_type    TEXT        NOT NULL CHECK (list_type IN ('include', 'exclude')),
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (segment_id, context_type, entry_key, list_type, created_at)
) PARTITION BY RANGE (created_at);

-- Set up pg_partman parent (only if extension exists)
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'pg_partman') THEN
        PERFORM partman.create_parent(
          p_parent_table    => 'public.segment_list_entries',
          p_control         => 'created_at',
          p_interval        => 'monthly',
          p_premake         => 3,
          p_start_partition => date_trunc('month', now())::text
        );

        -- Configure part_config for infinite retention
        UPDATE partman.part_config
           SET retention = NULL, infinite_time_partitions = true
         WHERE parent_table = 'public.segment_list_entries';
    ELSE
        -- Fallback: create a single default partition if partitioning is used but no partman
        -- Or just create one partition for now so it's usable.
        EXECUTE 'CREATE TABLE segment_list_entries_default PARTITION OF segment_list_entries DEFAULT';
    END IF;
END $$;

-- Index for lookup performance (segment_id, context_type, list_type are the common filters)
CREATE INDEX idx_segment_list_entries_lookup ON segment_list_entries (segment_id, context_type, list_type);
