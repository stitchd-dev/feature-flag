-- Migration 001_20260412: segment_rules
--
-- Rule-based segments store their rules here. 
-- rules are ordered by rule_index.

CREATE TABLE segment_rules (
  id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  segment_id  UUID NOT NULL REFERENCES segments(id) ON DELETE CASCADE,
  rule_index  INT  NOT NULL,
  rule_def    JSONB NOT NULL,
  UNIQUE (segment_id, rule_index)
);

CREATE INDEX idx_segment_rules_segment_id_rule_index ON segment_rules (segment_id, rule_index ASC);
