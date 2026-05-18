-- Drop the experiment_results table.
-- This table has been superseded by ClickHouse-backed storage in
-- stitchd-analytics-service.  All production consumers were removed in the
-- boundaries_20260518 refactor (waves 1–2).

DROP TABLE IF EXISTS experiment_results;
