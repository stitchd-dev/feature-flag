-- =============================================================================
-- Idempotency keys (platform_hardening_20260608, Phase 1). Purely additive.
--
-- Backs the gateway's Idempotency-Key middleware: a safe-retry ledger keyed by
-- (scope, idempotency_key). The gateway is the ONLY writer/reader of this table
-- — it is HTTP-edge cross-cutting state (request dedup), NOT domain data, which
-- is why it lives here as a standalone table the gateway touches directly via a
-- narrowly-scoped PgPool (documented in conductor/tech-stack.md).
--
-- Lifecycle per key:
--   1. claim   — INSERT a row with response_status = NULL (in-flight marker).
--   2. complete— UPDATE response_status/_body once the handler returns 2xx.
--   3. release — DELETE the row when the handler returns non-2xx (so the client
--                may legitimately retry the same key).
--   4. sweep   — a periodic DELETE removes rows older than the configured TTL.
--
-- A replay (same scope+key, completed) returns response_status/_body verbatim;
-- a key reused with a different request_hash is rejected 422 by the middleware.
-- =============================================================================

CREATE TABLE IF NOT EXISTS idempotency_keys (
    -- Caller-scoping: a one-way hash of the Authorization header (so two
    -- different actors reusing the same client-chosen key never collide), or a
    -- fixed sentinel for unauthenticated callers.
    scope                 TEXT        NOT NULL,
    -- The client-supplied `Idempotency-Key` request header value.
    idempotency_key       TEXT        NOT NULL,
    -- One-way fingerprint of (method + path + query + body). A second request
    -- reusing the key with a DIFFERENT fingerprint is a misuse → 422. The raw
    -- request body is never stored (privacy: NFR-1).
    request_hash          TEXT        NOT NULL,
    -- NULL while the first request is in-flight; set to the captured HTTP status
    -- once the handler returns a 2xx that is worth replaying.
    response_status       INT,
    -- Raw response bytes captured for verbatim replay (content-agnostic — stored
    -- as BYTEA rather than JSONB so any 2xx payload/content-type replays exactly).
    response_body         BYTEA,
    response_content_type TEXT,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (scope, idempotency_key)
);

-- Drives the TTL sweeper's `DELETE ... WHERE created_at < now() - ttl`.
CREATE INDEX IF NOT EXISTS idx_idempotency_keys_created_at
    ON idempotency_keys (created_at);
