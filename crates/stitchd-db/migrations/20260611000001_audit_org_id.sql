-- Audit log org-scoping (audit_log_20260611).
--
-- The audit_log table was created (v1 baseline) without an org_id, so audit
-- entries could not be scoped to an organisation for the admin Audit page.
-- Add a nullable org_id (system / no-org actions leave it NULL) plus a
-- keyset-friendly index for the newest-first, org-scoped read path
-- (ORDER BY created_at DESC, id DESC WHERE org_id = $1).

ALTER TABLE public.audit_log
    ADD COLUMN org_id uuid;

CREATE INDEX idx_audit_log_org_created
    ON public.audit_log (org_id, created_at DESC, id DESC);
