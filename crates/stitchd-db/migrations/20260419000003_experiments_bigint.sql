-- Migration 20260419000003: fix experiments column types
--
-- Experiments was created with INT columns for version and min_sample_size, but
-- all other tables in this schema use BIGINT (matching the i64 Rust structs).
-- Promote these columns to BIGINT for consistency and to remove lossy i64→i32
-- casts in the Postgres repository.

ALTER TABLE experiments
    ALTER COLUMN version TYPE BIGINT,
    ALTER COLUMN min_sample_size TYPE BIGINT;

ALTER TABLE experiment_iterations
    ALTER COLUMN min_sample_size TYPE BIGINT;
