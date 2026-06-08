# Plan: Platform Hardening

Implementation plan for `platform_hardening_20260608`. TDD per `conductor/workflow.md`
(Red → Green → Refactor → ≥90% coverage). Phases 1, 3, 4, 5 are independent
(`depends: -`) and can run as parallel worker-waves; Phase 2 depends on Phase 1's
idempotency store.

## Phase 1: Idempotency Store + REST Mutation Middleware
<!-- execution: parallel -->
<!-- depends: -->

- [x] Task 1: Migration — `idempotency_keys` PG table (`key`, `scope`, `request_hash`, `response_status`, `response_body jsonb`, `created_at`; unique `(scope, key)`) + regenerate sqlx offline cache [a00c79a]
  <!-- files: crates/stitchd-db/migrations, .sqlx -->
- [x] Task 2: Idempotency middleware — fingerprint (method+path+canonical body), replay stored 2xx, 422 on key-reuse-with-different-fingerprint, fail-open on store error — TDD [a00c79a]
  <!-- files: crates/stitchd-gateway/src/middleware/idempotency.rs, crates/stitchd-gateway/src/middleware/mod.rs -->
  <!-- depends: task1 -->
- [x] Task 3: TTL sweeper tokio task + `STITCHD_GATEWAY_IDEMPOTENCY_TTL_SECS` env var (default 86400) + env-vars doc [a00c79a]
  <!-- files: crates/stitchd-gateway/src/main.rs, crates/stitchd-gateway/src/state.rs -->
  <!-- depends: task1 -->
- [x] Task 4: Wire layer onto all mutating `/v1/*` routes + integration tests (AC-1..4) [a00c79a]
  <!-- files: crates/stitchd-gateway/src/router.rs, crates/stitchd-gateway/src/tests -->
  <!-- depends: task2 -->
- [ ] Task: Conductor - User Manual Verification 'Idempotency Store + REST Mutation Middleware' (Protocol in workflow.md)

## Phase 2: SDK gRPC / Event-Ingest Idempotency
<!-- depends: phase1 -->

- [ ] Task 1: Per-batch idempotency key on event-ingest (`/v1/sdk/events:batch` REST + gRPC ingest) — server-side dedup (ledger or ReplacingMergeTree dedup-key) — TDD
  <!-- files: crates/stitchd-gateway/src/routes/sdk_backend.rs, crates/stitchd-event-writer/src, proto -->
- [ ] Task 2: Rust SDK stamps each flush batch with a stable idempotency key — at-least-once flush becomes exactly-once at the server
  <!-- files: sdks/rust/src/events.rs, sdks/rust/src/event_buffer.rs, sdks/rust/src/client.rs -->
  <!-- depends: task1 -->
- [ ] Task 3: Live-CH integration test — duplicate batch replay does not double-count (AC-6)
  <!-- files: crates/stitchd-gateway/src/tests, sdks/rust/tests -->
  <!-- depends: task1 -->
- [ ] Task: Conductor - User Manual Verification 'SDK gRPC / Event-Ingest Idempotency' (Protocol in workflow.md)

## Phase 3: On-Demand Interaction Recompute (feature-flag-uga)
<!-- depends: -->

- [x] Task 1: Wire CH reader/writer + interaction repo into the on-demand `ExperimentRecomputer` / `StatsServiceImpl` [f29420d]
  <!-- files: crates/stitchd-stats-service/src/grpc/service.rs, crates/stitchd-stats-service/src/recompute_trigger.rs, crates/stitchd-stats-service/src/main.rs -->
- [x] Task 2: Call `run_interaction_sweep` in the on-demand recompute path (order-capped via `STITCHD_STATS_MAX_INTERACTION_ORDER`); sweep failure marks job failed — TDD [f29420d]
  <!-- files: crates/stitchd-stats-service/src/grpc/service.rs, crates/stitchd-stats-service/src/interaction_compute.rs -->
  <!-- depends: task1 -->
- [x] Task 3: Unit tests for the on-demand sweep seam (no-op vs proceed); live e2e infeasible (needs experimentation-service gRPC) so no ci.yml --test change; close feature-flag-uga [f29420d]
  <!-- files: crates/stitchd-stats-service/tests, .github/workflows/ci.yml -->
  <!-- depends: task2 -->
- [ ] Task: Conductor - User Manual Verification 'On-Demand Interaction Recompute' (Protocol in workflow.md)

## Phase 4: Cursor-Based Pagination Migration
<!-- depends: -->

- [ ] Task 1: Document the cursor contract in `tech-stack.md` (reverses domain_boundaries page-based canonical) — opaque keyset cursor, `?cursor=&limit=`, `{items, next_cursor}`
  <!-- files: conductor/tech-stack.md -->
- [ ] Task 2: Shared cursor primitives — `CursorParams` + `CursorPage<T>` + opaque-token encode/decode (base64 keyset) + proto messages — TDD
  <!-- files: crates/stitchd-gateway/src/pagination.rs, proto -->
  <!-- depends: task1 -->
- [ ] Task 3: Migrate repo queries to keyset pagination (replace OFFSET + COUNT(*) OVER()) — TDD per repo
  <!-- files: crates/stitchd-db/src/repository -->
  <!-- depends: task2 -->
- [ ] Task 4: Migrate gateway list routes + OpenAPI contract surface
  <!-- files: crates/stitchd-gateway/src/routes, crates/stitchd-gateway/src/openapi.rs -->
  <!-- depends: task3 -->
- [ ] Task 5: Migrate Admin UI list views to cursor (next/prev tokens, drop page numbers) + vitest
  <!-- files: admin/src -->
  <!-- depends: task4 -->
- [ ] Task: Conductor - User Manual Verification 'Cursor-Based Pagination Migration' (Protocol in workflow.md)

## Phase 5: Fresh-DB Reset Tooling (feature-flag-7rp)
<!-- execution: parallel -->
<!-- depends: -->

- [x] Task 1: `scripts/reset_dev_db.sh` — drop + recreate + migrate from V1 baseline; non-interactive, idempotent; resolves baseline checksum drift [e604ccb]
  <!-- files: crates/xtask/src, scripts -->
- [x] Task 2: Document the fresh-DB verification flow (matches CI fresh-from-scratch) + close feature-flag-7rp [84bb55b]
  <!-- files: README.md, conductor/workflow.md -->
- [ ] Task: Conductor - User Manual Verification 'Fresh-DB Reset Tooling' (Protocol in workflow.md)
