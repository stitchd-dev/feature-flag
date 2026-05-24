-- Migration: add optional name to sdk_keys
-- Existing rows default to empty string; callers may update via management API.

ALTER TABLE sdk_keys ADD COLUMN IF NOT EXISTS name TEXT NOT NULL DEFAULT '';
