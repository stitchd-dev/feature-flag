-- Migration 0005: rename is_disabled → targeting_on and add matched_rule_id
-- to flag_evaluation_log and flag_evaluation_log_v2.
--
-- Background: EvalLogRow struct was updated to use `targeting_on` (positively
-- framed: true = flag targeting is ON) and `matched_rule_id` for experiment
-- exposure attribution, but the ClickHouse schema was never updated to match.

ALTER TABLE flag_evaluation_log
    RENAME COLUMN is_disabled TO targeting_on;

ALTER TABLE flag_evaluation_log
    ADD COLUMN IF NOT EXISTS matched_rule_id Nullable(UUID);

ALTER TABLE flag_evaluation_log_v2
    RENAME COLUMN is_disabled TO targeting_on;

ALTER TABLE flag_evaluation_log_v2
    ADD COLUMN IF NOT EXISTS matched_rule_id Nullable(UUID);
