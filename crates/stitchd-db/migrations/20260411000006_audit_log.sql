-- Migration 006: audit_log
--
-- Append-only audit log for all mutations.
-- No updated_at, deleted_at, or version — records are never modified.

CREATE TABLE audit_log (
    id             UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    actor_id       UUID,                              -- NULL for system actions
    resource_type  TEXT        NOT NULL,
    resource_id    UUID        NOT NULL,
    action         TEXT        NOT NULL,
    diff           JSONB       NOT NULL DEFAULT '{}',
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Support queries: "show all changes to resource X", "show all actions by actor Y",
-- "show recent activity" (time-range scans).
CREATE INDEX idx_audit_log_resource ON audit_log(resource_type, resource_id);
CREATE INDEX idx_audit_log_actor_id ON audit_log(actor_id);
CREATE INDEX idx_audit_log_created_at ON audit_log(created_at);
