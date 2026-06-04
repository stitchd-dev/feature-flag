-- =============================================================================
-- Lifecycle automation — scheduled changes, flag prerequisites, dependency graph
-- (flag_lifecycle_20260604, Phase 1 Task 2). Purely additive.
--
-- Adds:
--   * scheduled_changes        — one-shot / recurring mutations against an entity
--   * scheduled_change_runs     — per-fire run history
--   * flag_prerequisites        — flag→flag prerequisite gate edges
--     (+ fallback_variant_id column on feature_flags)
--   * entity_dependencies       — generic cross-entity edge table for referential
--                                 integrity ("who depends on me" delete-block lookups)
-- =============================================================================

-- ---------------------------------------------------------------------------
-- scheduled_changes
--   Entity-agnostic scheduled mutation. mutation_payload is the JSON-encoded
--   request the owning service's mutation/lifecycle RPC consumes at fire time.
--   One-shot changes carry scheduled_at; recurring changes carry rrule + tz.
--   All instants are canonical UTC; tz is the IANA zone the author authored in.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS scheduled_changes (
    id               UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    entity_type      TEXT        NOT NULL
                     CONSTRAINT chk_scheduled_changes_entity_type
                     CHECK (entity_type IN ('flag', 'segment', 'experiment')),
    entity_id        UUID        NOT NULL,
    env_id           UUID        NOT NULL,
    mutation_payload JSONB       NOT NULL,
    schedule_kind    TEXT        NOT NULL
                     CONSTRAINT chk_scheduled_changes_schedule_kind
                     CHECK (schedule_kind IN ('one_shot', 'recurring')),
    -- One-shot: the instant to fire (UTC). Null for recurring.
    scheduled_at     TIMESTAMPTZ,
    -- Recurring: RFC-5545 RRULE string + the IANA timezone it is evaluated in.
    rrule            TEXT,
    tz               TEXT,
    -- Next computed firing instant (UTC); the scheduler queries this.
    next_run_at      TIMESTAMPTZ,
    -- Last time this change actually fired (UTC).
    last_run_at      TIMESTAMPTZ,
    status           TEXT        NOT NULL DEFAULT 'pending'
                     CONSTRAINT chk_scheduled_changes_status
                     CHECK (status IN ('pending', 'active', 'paused', 'applied', 'failed', 'cancelled')),
    version          BIGINT      NOT NULL DEFAULT 1,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Author of the scheduled change (user UUID), or NULL when system-created.
    created_by       UUID,
    deleted_at       TIMESTAMPTZ
);

-- Due-change query support: only pending/active rows are ever due to fire.
CREATE INDEX IF NOT EXISTS idx_scheduled_changes_due
    ON scheduled_changes (next_run_at)
    WHERE status IN ('pending', 'active');

-- Per-entity / per-env listing (UI: "schedules on this flag").
CREATE INDEX IF NOT EXISTS idx_scheduled_changes_entity
    ON scheduled_changes (entity_type, entity_id) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_scheduled_changes_env
    ON scheduled_changes (env_id) WHERE deleted_at IS NULL;

-- ---------------------------------------------------------------------------
-- scheduled_change_runs
--   Append-only per-fire history for a scheduled change.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS scheduled_change_runs (
    id                  UUID        NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    scheduled_change_id UUID        NOT NULL REFERENCES scheduled_changes(id),
    fired_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    outcome             TEXT        NOT NULL
                        CONSTRAINT chk_scheduled_change_runs_outcome
                        CHECK (outcome IN ('applied', 'skipped', 'failed')),
    -- Free-text reason / error detail (e.g. flag-locked skip reason).
    detail              TEXT
);

CREATE INDEX IF NOT EXISTS idx_scheduled_change_runs_change_fired
    ON scheduled_change_runs (scheduled_change_id, fired_at);

-- ---------------------------------------------------------------------------
-- flag_prerequisites
--   flag→flag prerequisite gate edges. The dependent flag (flag_id) requires
--   the prerequisite flag (prerequisite_flag_id) to resolve to required_variant_id.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS flag_prerequisites (
    id                   UUID NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    flag_id              UUID NOT NULL REFERENCES feature_flags(id) ON DELETE CASCADE,
    prerequisite_flag_id UUID NOT NULL REFERENCES feature_flags(id),
    -- The variant the prerequisite flag must resolve to for the gate to pass.
    required_variant_id  UUID NOT NULL,
    -- Stable ordering for display / deterministic evaluation.
    "order"              INT  NOT NULL DEFAULT 0,
    UNIQUE (flag_id, prerequisite_flag_id)
);

CREATE INDEX IF NOT EXISTS idx_flag_prerequisites_flag
    ON flag_prerequisites (flag_id);
-- "which flags require me" — needed for the delete-block lookup.
CREATE INDEX IF NOT EXISTS idx_flag_prerequisites_prerequisite
    ON flag_prerequisites (prerequisite_flag_id);

-- The variant returned when a prerequisite gate fails. Nullable: when unset the
-- gate falls back to the flag's off/disabled variant.
ALTER TABLE feature_flags
    ADD COLUMN IF NOT EXISTS fallback_variant_id UUID REFERENCES variants(id);

-- ---------------------------------------------------------------------------
-- entity_dependencies
--   Generic directed edge table for cross-entity referential integrity. A row
--   means "from_entity depends on to_entity". The (to_type, to_id) index powers
--   the "who depends on me" query that blocks deletion/archival of a referenced
--   entity (409 DEPENDENCY_EXISTS).
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS entity_dependencies (
    id        UUID NOT NULL DEFAULT gen_random_uuid() PRIMARY KEY,
    from_type TEXT NOT NULL,
    from_id   UUID NOT NULL,
    to_type   TEXT NOT NULL,
    to_id     UUID NOT NULL,
    -- Edge kind, e.g. prerequisite | segment_ref | experiment_binding.
    kind      TEXT NOT NULL,
    UNIQUE (from_type, from_id, to_type, to_id, kind)
);

-- "who depends on me" — the delete-block query.
CREATE INDEX IF NOT EXISTS idx_entity_dependencies_target
    ON entity_dependencies (to_type, to_id);
