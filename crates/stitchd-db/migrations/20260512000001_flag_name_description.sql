-- Migration 20260512000001: add name and description to feature_flags
--
-- Adds human-readable display fields to flags for the admin UI.
-- Both default to empty string so existing flags remain valid.

ALTER TABLE feature_flags
    ADD COLUMN name        TEXT NOT NULL DEFAULT '',
    ADD COLUMN description TEXT NOT NULL DEFAULT '';
