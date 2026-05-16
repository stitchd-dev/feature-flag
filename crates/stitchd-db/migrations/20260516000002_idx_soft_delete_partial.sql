-- FR-3: Partial indexes on soft-delete columns.
--
-- Every admin list query filters WHERE deleted_at IS NULL. Without a partial
-- index, PostgreSQL must visit all rows including soft-deleted ones, even
-- though deleted rows are typically a small fraction of the table.
--
-- Each partial index uses WHERE deleted_at IS NULL so only live rows are
-- indexed, reducing index size and improving list-query performance.
--
-- Production note: for zero-downtime deployment use CREATE INDEX CONCURRENTLY
-- for each statement and run them individually in autocommit mode.

CREATE INDEX IF NOT EXISTS idx_feature_flags_project_active
    ON feature_flags(project_id)
    WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_segments_environment_active
    ON segments(environment_id)
    WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_projects_organisation_active
    ON projects(organisation_id)
    WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_environments_project_active
    ON environments(project_id)
    WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_event_definitions_environment_active
    ON event_definitions(environment_id)
    WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_experiments_env_active
    ON experiments(env_id)
    WHERE deleted_at IS NULL;
