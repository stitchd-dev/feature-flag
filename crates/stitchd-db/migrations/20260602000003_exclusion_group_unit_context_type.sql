-- Adds the diversion (randomization) unit context type to exclusion groups.
--
-- HIGH-1 fix: mutual exclusion only holds when every member experiment of a
-- group randomizes on the SAME context unit, because the exclusion bucket is
-- hash(group_salt + that unit's context key). Pinning the unit at the group
-- level (and validating member experiments against it on assignment) guarantees
-- a single shared bucket axis across the group's flags.
--
-- Existing rows default to 'user' (the platform default randomization unit).
ALTER TABLE exclusion_groups
    ADD COLUMN IF NOT EXISTS unit_context_type TEXT NOT NULL DEFAULT 'user';
