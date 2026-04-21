-- Mark an organisation as the immutable platform-owned "System" org.
-- The System org is created during superadmin bootstrap and must never be
-- deleted, renamed, or used to host customer projects/users.
ALTER TABLE organisations
    ADD COLUMN is_system BOOLEAN NOT NULL DEFAULT false;
