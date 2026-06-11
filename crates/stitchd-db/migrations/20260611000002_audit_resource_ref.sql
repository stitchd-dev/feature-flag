-- Audit log edge-capture support (audit_log_20260611).
--
-- The original audit_log assumed per-resource writes where the affected entity's
-- UUID is always known. Gateway-edge capture often does not have a UUID (e.g.
-- collection-level creates, or flag routes keyed by string key), so:
--   * resource_id becomes nullable (set only when the path id parses as a UUID);
--   * resource_ref (text) holds the human path reference (flag key / UUID / NULL
--     for collection creates) so the Audit UI has something to display.

ALTER TABLE public.audit_log
    ALTER COLUMN resource_id DROP NOT NULL;

ALTER TABLE public.audit_log
    ADD COLUMN resource_ref text;
