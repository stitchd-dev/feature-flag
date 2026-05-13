-- Add a bare ConditionExpr column to segments for rule-based segment UI editing.
--
-- Rule-based segments can optionally store a single top-level ConditionExpr
-- (e.g. {"And": [...]}) separate from the segment_rules rows. This is the value
-- the admin condition builder reads and writes.
--
-- Nullable: existing rule segments have no condition_expr yet (NULL = not set).
-- List-based segments leave this NULL always.
ALTER TABLE segments ADD COLUMN IF NOT EXISTS condition_expr JSONB;
