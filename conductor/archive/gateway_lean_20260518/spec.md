# Gateway Lean Refactor

## Overview

The `stitchd-gateway` has accumulated responsibilities beyond its core mandate of
authentication verification and request routing. It currently holds a live ClickHouse
client (context intelligence, eval-stats) and a PostgreSQL pool (context-registry),
making it a mixed-concern service. Additionally, `event-service` is a thin
ClickHouse-write wrapper with no domain logic of its own. This track strips gateway
DB connections, folds `event-service` into a new `analytics-service`, and moderately
trims heavy route handlers.

## Functional Requirements

### 1. New `stitchd-analytics-service` crate (replaces `stitchd-event-service`)
Owns all ClickHouse interaction and context registry:

**Ingestion (from event-service):**
- `IngestEvents(events[])` gRPC method — writes events to ClickHouse

**Context registry (from gateway PG direct):**
- `RegisterContext(env_id, context_type, param, inferred_type)` — write
- `ListContextTypes(env_id)` + `ListContextParams(env_id, type)` — read (autocomplete)

**Analytics reads (from gateway context_intel + eval_stats):**
- `GetEvalStats(env_id, flag_key, window)` — ClickHouse eval log stats
- `GetContextIntelligence(env_id, context_type)` — usage histograms / param suggestions

### 2. `stitchd-event-service` crate is retired
- Code folded into `stitchd-analytics-service`
- Proto package `events.v1` kept for SDK backwards-compat, re-exported from `analytics.v1`
- Port `50054` reassigned to analytics-service

### 3. Gateway drops all database connections
- Remove `ch_client`, `context_registry`, `pg_pool` from `GatewayState`
- Replace `event_client: EventIngestionServiceClient` with `analytics_client`
- `context_intel.rs` and `eval_stats.rs` become thin gRPC passthrough handlers
- Event ingestion route proxies to `analytics_client`
- Graceful degradation: return empty/default if analytics-service is unreachable

### 4. Moderate route handler trim
- Handlers contain only: auth check → proto call → JSON serialize
- Extract shared pagination/error-mapping helpers into `routes/mod.rs`
- Remove dead code, unused imports, stale comments
- No business logic moves

## Non-Functional Requirements

- `cargo tree -p stitchd-gateway | grep -E "clickhouse|sqlx"` must return nothing
- `analytics-service` starts on port `50054` (reuses event-service port)
- All existing HTTP API routes remain unchanged — no breaking changes
- Gateway degrades gracefully if analytics-service is unreachable

## Acceptance Criteria

- [ ] `cargo tree -p stitchd-gateway | grep -E "clickhouse|sqlx"` returns nothing
- [ ] `stitchd-event-service` crate is removed from the workspace
- [ ] `stitchd-analytics-service` passes integration tests covering ingestion, context registry, and analytics reads
- [ ] All existing gateway route tests still pass
- [ ] `GatewayState` has no DB client fields
- [ ] `context_intel`, `eval_stats`, and event ingestion routes respond correctly via gRPC passthrough

## Out of Scope

- Merging with `stats-service` (stats computes experiment results — different domain)
- Changing any existing SDK-facing proto contracts
- Moving experiment stats computation
- UI changes
