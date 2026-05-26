-- Allow re-creating a flag with the same key after soft-deletion.
-- The old table-level UNIQUE (key, project_id) constraint blocks key re-use
-- when deleted_at IS NOT NULL. Replace it with a partial unique index that
-- only enforces uniqueness among live (non-deleted) flags.
ALTER TABLE feature_flags DROP CONSTRAINT IF EXISTS feature_flags_key_project_id_key;

CREATE UNIQUE INDEX IF NOT EXISTS feature_flags_key_project_id_live
    ON feature_flags (key, project_id)
    WHERE deleted_at IS NULL;
