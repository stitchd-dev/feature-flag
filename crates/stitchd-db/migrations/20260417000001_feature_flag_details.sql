-- Migration 20260417000001: feature_flag_details
--
-- Refines feature_flags with hashing configuration, ordering, and rule integration.

-- ---------------------------------------------------------------------------
-- flag_hashing_config
-- ---------------------------------------------------------------------------
-- Configures which context parameters are used for hashing a specific flag.
CREATE TABLE flag_hashing_config (
    id          UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    flag_id     UUID        NOT NULL REFERENCES feature_flags(id) ON DELETE CASCADE,
    parameter_key TEXT      NOT NULL,
    parameter_type TEXT     NOT NULL, -- 'user', 'session', 'custom', etc.
    "order"     INT         NOT NULL DEFAULT 0,
    UNIQUE (flag_id, parameter_key, parameter_type)
);

CREATE INDEX idx_flag_hashing_config_flag_id ON flag_hashing_config(flag_id);

-- ---------------------------------------------------------------------------
-- feature_flag_rules
-- ---------------------------------------------------------------------------
-- Rules for flag evaluation, evaluated in rule_index order.
CREATE TABLE feature_flag_rules (
    id          UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    flag_id     UUID        NOT NULL REFERENCES feature_flags(id) ON DELETE CASCADE,
    rule_index  INT         NOT NULL,
    rule_def    JSONB       NOT NULL, -- The Rule Engine's rule definition
    variant_id  UUID        NOT NULL REFERENCES variants(id), -- The variant to return if rule matches
    UNIQUE (flag_id, rule_index)
);

CREATE INDEX idx_feature_flag_rules_flag_id ON feature_flag_rules(flag_id);

-- ---------------------------------------------------------------------------
-- Update feature_flags
-- ---------------------------------------------------------------------------
-- Add default_variant_id to feature_flags.
ALTER TABLE feature_flags ADD COLUMN default_variant_id UUID REFERENCES variants(id);

CREATE INDEX idx_feature_flags_default_variant_id ON feature_flags(default_variant_id);

-- ---------------------------------------------------------------------------
-- Audit Logging Triggers (if central audit is active)
-- ---------------------------------------------------------------------------
-- Assuming central audit logic from 20260411000006_audit_log.sql
-- If central audit logic exists, I should apply it here if needed.
-- But the task "Ensure audit logging triggers are active for new tables" is separate.
