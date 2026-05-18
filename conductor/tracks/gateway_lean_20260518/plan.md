# Implementation Plan: Gateway Lean Refactor

## Phase 1: New `stitchd-analytics-service` Scaffold

- [ ] Task 1: Create crate skeleton
  - Add `crates/stitchd-analytics-service/` to workspace `Cargo.toml`
  - Scaffold `Cargo.toml`, `src/main.rs`, `src/lib.rs`, `src/config.rs`
  - Dependencies: tonic, tokio, clickhouse, sqlx, tracing, anyhow

- [ ] Task 2: Define `analytics.v1` proto package
  - Add `proto/analytics/v1/analytics.proto`
  - Define RPCs: `IngestEvents`, `RegisterContext`, `ListContextTypes`,
    `ListContextParams`, `GetEvalStats`, `GetContextIntelligence`
  - Re-export `events.v1.IngestEvents` alias for SDK backwards-compat
  - Regenerate proto bindings

- [ ] Task 3: gRPC server skeleton + config
  - `src/grpc/mod.rs` + `src/grpc/service.rs` with stub `AnalyticsServiceImpl`
  - `src/config.rs`: `ANALYTICS_GRPC_PORT` (default 50054), `DATABASE_URL`,
    `CLICKHOUSE_*` env vars
  - Wire up in `main.rs`, bind port 50054
  - Write compile-smoke test

- [ ] Task: Conductor - User Manual Verification 'New analytics-service Scaffold' (Protocol in workflow.md)

## Phase 2: Event Ingestion Migration

- [ ] Task 1: Move event ingestion gRPC handler
  - Copy `event-service/src/grpc/event_ingestion.rs` →
    `analytics-service/src/grpc/event_ingestion.rs`
  - Adapt imports; register on `AnalyticsServiceImpl`
  - Port all existing tests from event-service

- [ ] Task 2: Verify parity + coverage
  - Run `cargo test -p stitchd-analytics-service` — all ingestion tests pass
  - Coverage ≥ 90% on new module

- [ ] Task: Conductor - User Manual Verification 'Event Ingestion Migration' (Protocol in workflow.md)

## Phase 3: Context Registry Migration

- [ ] Task 1: Move context registry repository + PG logic
  - Copy `event-service/src/registry.rs` →
    `analytics-service/src/context_registry.rs`
  - Implement `RegisterContext`, `ListContextTypes`, `ListContextParams` gRPC handlers
  - Wire PG pool in analytics-service `main.rs`

- [ ] Task 2: Tests
  - Port existing registry tests from event-service
  - Add tests for all three new gRPC methods

- [ ] Task: Conductor - User Manual Verification 'Context Registry Migration' (Protocol in workflow.md)

## Phase 4: Analytics Reads Migration

- [ ] Task 1: Move eval stats ClickHouse queries
  - Copy gateway's `eval_stats.rs` query logic →
    `analytics-service/src/eval_stats.rs`
  - Implement `GetEvalStats` gRPC handler

- [ ] Task 2: Move context intelligence ClickHouse queries
  - Copy gateway's `context_intel.rs` query logic →
    `analytics-service/src/context_intel.rs`
  - Implement `GetContextIntelligence` gRPC handler

- [ ] Task 3: Tests
  - Unit tests for both query modules (mock ClickHouse client)
  - Integration tests against live ClickHouse

- [ ] Task: Conductor - User Manual Verification 'Analytics Reads Migration' (Protocol in workflow.md)

## Phase 5: Gateway Lean-Up
<!-- depends: phase2, phase3, phase4 -->

- [ ] Task 1: Remove DB clients from `GatewayState`
  - Drop `ch_client`, `context_registry` fields
  - Remove `pg_pool` parameter from `GatewayState::connect()`
  - Add `analytics_client: Arc<Mutex<AnalyticsServiceClient<Channel>>>`
  - Update `from_channels()` test constructor

- [ ] Task 2: Rewire `context_intel.rs` and `eval_stats.rs` as thin passthrough
  - Replace direct ClickHouse calls with `analytics_client` gRPC calls
  - Graceful degradation: return empty/default on `Unavailable` status

- [ ] Task 3: Rewire event ingestion route to `analytics_client`
  - `routes/events.rs`: swap `event_client` → `analytics_client`
  - Remove `event_client` field from `GatewayState`

- [ ] Task 4: Rewire context registry call sites
  - All `context_registry.upsert_*` calls → `analytics_client.register_context()`
  - Best-effort: log errors, don't fail the request

- [ ] Task 5: Verify `cargo tree` clean
  - `cargo tree -p stitchd-gateway | grep -E "clickhouse|sqlx"` must return nothing
  - All existing gateway tests pass

- [ ] Task: Conductor - User Manual Verification 'Gateway Lean-Up' (Protocol in workflow.md)

## Phase 6: Retire `stitchd-event-service`
<!-- depends: phase5 -->

- [ ] Task 1: Remove event-service from workspace
  - Delete `crates/stitchd-event-service/`
  - Remove from workspace `Cargo.toml` members
  - Remove from `docker-compose.yml`
  - Update CI workflow files that reference event-service

- [ ] Task 2: Update startup documentation + scripts
  - Update any README / dev-startup scripts
  - Verify full `cargo build --workspace` succeeds

- [ ] Task: Conductor - User Manual Verification 'Retire event-service' (Protocol in workflow.md)

## Phase 7: Route Handler Moderate Trim
<!-- depends: -->

- [ ] Task 1: Extract shared helpers in `routes/mod.rs`
  - Pagination helpers, error-mapping utilities currently duplicated across handlers
  - No logic changes — pure extraction

- [ ] Task 2: Trim `flags.rs`
  - Remove dead code, unused imports, stale comments
  - Enforce: no inline DB calls, no analytics calls

- [ ] Task 3: Trim `segments.rs`
  - Same as Task 2 for segments

- [ ] Task 4: Clippy + fmt pass
  - `cargo clippy -p stitchd-gateway -- -D warnings`
  - `cargo fmt -p stitchd-gateway --check`

- [ ] Task: Conductor - User Manual Verification 'Route Handler Trim' (Protocol in workflow.md)

## Phase 8: Final Verification
<!-- depends: phase6, phase7 -->

- [ ] Task 1: Full workspace build + test
  - `cargo test --workspace`
  - `cargo clippy --workspace --all-targets -- -D warnings`

- [ ] Task 2: Acceptance criteria audit
  - `cargo tree -p stitchd-gateway | grep -E "clickhouse|sqlx"` → empty ✓
  - `stitchd-event-service` crate absent from workspace ✓
  - All gateway route tests pass ✓
  - `GatewayState` has no DB fields ✓

- [ ] Task: Conductor - User Manual Verification 'Final Verification' (Protocol in workflow.md)
