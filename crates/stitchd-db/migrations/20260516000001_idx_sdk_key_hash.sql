-- FR-1: Composite index on sdk_keys(key_hash, is_active).
--
-- The authentication hot path queries:
--   WHERE key_hash = $1 AND is_active = TRUE
-- The existing index idx_sdk_keys_environment_active(environment_id, is_active)
-- forces a full table scan on every SDK evaluation. This composite index covers
-- both predicate columns and enables index-only scans on key_hash lookups.
--
-- Production note: for zero-downtime deployment run this manually instead:
--   CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_sdk_keys_key_hash_active
--       ON sdk_keys(key_hash, is_active);
CREATE INDEX IF NOT EXISTS idx_sdk_keys_key_hash_active
    ON sdk_keys(key_hash, is_active);
