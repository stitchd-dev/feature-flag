-- 0001_keyspace: Create the stitchd keyspace.
-- Replication factor is set via env at runtime by the applier's bootstrap step;
-- this file is a no-op placeholder because keyspace creation happens in bootstrap().
-- Kept for audit-trail completeness: version 0001 marks "keyspace bootstrapped".
SELECT now() FROM system.local;
