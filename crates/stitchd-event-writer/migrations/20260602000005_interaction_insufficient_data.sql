-- MED-2 fix: surface insufficient-data interaction rows explicitly.
--
-- When a cell sample is too small for a valid test, interaction_estimate and
-- p_value are persisted as 0.0 sentinels (ClickHouse Float64 cannot hold NaN).
-- Without an explicit flag, a UI shows p_value = 0.0 which reads as MAXIMALLY
-- significant. This column lets readers/consumers treat such rows as
-- non-actionable and ignore the sentinel estimate/p_value.
ALTER TABLE experiment_interactions
    ADD COLUMN IF NOT EXISTS insufficient_data Bool DEFAULT false;
