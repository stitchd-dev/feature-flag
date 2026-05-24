# Bugs: integration_bugfix_20260524

Discovered during Phase 1 (Stack Bringup) and subsequent discovery phases.

---

## Critical

*(none yet)*

---

## High

### BUG-001: `STITCHD_AUTH_ENCRYPTION_KEY` missing from docker-compose.yml
- **Phase discovered:** Phase 1 — stack bringup
- **Component:** `docker-compose.yml`, `stitchd-auth-service`
- **Reproduction:** `docker compose up` (or start auth-service without the env var set)
- **Expected:** auth-service starts and binds gRPC port 50051
- **Actual:** auth-service panics on startup: `STITCHD_AUTH_ENCRYPTION_KEY must be set: MissingEnvVar`
- **Root cause:** `docker-compose.yml` does not define `STITCHD_AUTH_ENCRYPTION_KEY` for the auth-service container, so the env var is absent when running via `docker compose`. Also missing from `.env.example`.
- **Fix:** Add `STITCHD_AUTH_ENCRYPTION_KEY` with a dev default (base64-encoded 32-byte key) to `docker-compose.yml` auth-service environment block; add to `.env.example` with a note that production deployments must generate their own key.

### BUG-002: stats-service default gRPC ports for experimentation/analytics are swapped
- **Phase discovered:** Phase 1 — stack bringup
- **Component:** `crates/stitchd-stats-service/src/config.rs`
- **Reproduction:** Start `stitchd-stats-service` without setting `STITCHD_EXPERIMENTATION_SERVICE_GRPC_URL` or `STITCHD_ANALYTICS_SERVICE_GRPC_URL`
- **Expected:** experimentation defaults to `http://localhost:50055`, analytics defaults to `http://localhost:50054`
- **Actual:** experimentation defaults to `http://localhost:50054` (analytics port!), analytics defaults to `http://localhost:50055` (experimentation port!) → both gRPC connections land on the wrong service → runtime errors on every stats computation
- **Root cause:** The `unwrap_or_else` default strings in `StatsConfig::from_env()` have the port numbers transposed.
- **Fix:** Swap the defaults in `config.rs`.

---

## Medium

### BUG-003: stats-service context_refresher queries stale table `flag_evaluation_log` instead of `flag_evaluation_log_v2`
- **Phase discovered:** Phase 1 — stack bringup
- **Component:** `crates/stitchd-stats-service/src/context_refresher.rs`
- **Reproduction:** Start stats-service; observe ERROR log: `Unknown table expression identifier 'flag_evaluation_log'`
- **Expected:** Context registry refreshes successfully from ClickHouse eval log
- **Actual:** ClickHouse returns error `Code: 60 ... Unknown table expression identifier 'flag_evaluation_log'` — the table was renamed to `flag_evaluation_log_v2` in migration `0004_flag_evaluation_log_v2.sql`, but `context_refresher.rs` still queries the old name
- **Root cause:** Stale table reference. The `ClickHouseEvalLogSource` SQL uses `FROM flag_evaluation_log` instead of `FROM flag_evaluation_log_v2`.
- **Fix:** Updated the SQL in `context_refresher.rs` to reference `flag_evaluation_log_v2`. Commit: `a7bc8f5`
- **Status:** ✅ FIXED

---

## Low

*(none yet)*

---
