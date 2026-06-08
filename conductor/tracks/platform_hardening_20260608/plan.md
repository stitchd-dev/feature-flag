# Plan: Platform Hardening

> **Last Revised: 2026-06-08 (Revision #1)** — Phase 4 reframed to the as-built
> cursor approach (opaque encoded-offset at the gateway; true keyset deferred to
> `feature-flag-cj5`) and scoped to top-level resource collections (detail
> sub-lists stay page-based). See `revisions.md` / `spec.md` FR-4.

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

- [x] Task 1: Per-batch idempotency key on event-ingest (`/v1/sdk/events:batch` REST + gRPC ingest) — server-side dedup (ledger or ReplacingMergeTree dedup-key) — TDD [f8d6d91]
  <!-- files: crates/stitchd-gateway/src/routes/sdk_backend.rs, crates/stitchd-event-writer/src, proto -->
- [x] Task 2: Rust SDK stamps each flush batch with a stable idempotency key — at-least-once flush becomes exactly-once at the server [f8d6d91]
  <!-- files: sdks/rust/src/events.rs, sdks/rust/src/event_buffer.rs, sdks/rust/src/client.rs -->
  <!-- depends: task1 -->
- [x] Task 3: Live-CH integration test — duplicate batch replay does not double-count (AC-6) [f8d6d91]
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
<!-- SCOPE (Revision #1): cursor = the 8 top-level resource collections (flags,
     experiments, segments, events, metrics, sdk-keys, org-users mgmt+admin,
     exclusion-groups). Experiment-detail sub-lists (iterations, exposures) stay
     page-based — they back numbered detail views and the exposure-count stat
     needs the `total` the cursor envelope omits. Approach: opaque encoded-offset
     at the gateway over existing offset-based service RPCs (zero proto/repo
     churn); true keyset internals deferred to feature-flag-cj5. -->

- [x] Task 1: Document the cursor contract in `tech-stack.md` (supersedes domain_boundaries page-based canonical) — opaque cursor, `?cursor=&limit=`, `{items, next_cursor}` [67c76f8, 734ff03]
  <!-- files: conductor/tech-stack.md -->
- [x] Task 2: Shared gateway cursor primitives — `CursorParams` + `CursorPage<T>` + `encode_cursor`/`decode_cursor` (opaque base64url token) — TDD [67c76f8]
  <!-- files: crates/stitchd-gateway/src/pagination.rs -->
  <!-- depends: task1 -->
- [x] Task 3: (Revised) Cursor transport via opaque encoded-offset at the gateway over each service's existing `(offset,limit)→(items,total)` RPC (`CursorParams::offset()` + `CursorPage::from_offset`) — TDD; NO proto/repo change [734ff03]
  <!-- files: crates/stitchd-gateway/src/pagination.rs -->
  <!-- depends: task2 -->
- [x] Task 4: Migrate the 8 top-level gateway list routes to cursor; OpenAPI contract surface (contract-check verifies method+path, unchanged) [229c0ab, 66b9568]
  <!-- files: crates/stitchd-gateway/src/routes, crates/stitchd-gateway/src/openapi.rs -->
  <!-- depends: task3 -->
- [x] Task 5: Migrate Admin UI list views to cursor (shared `usePaginatedList` + `Pagination`, next/prev, no page numbers) + vitest [66b9568]
  <!-- files: admin/src -->
  <!-- depends: task4 -->
- [x] Task: Conductor - User Manual Verification 'Cursor-Based Pagination Migration' — verified CI-green (gateway 288 + admin vitest 994, OpenAPI contract 23/120, docs idempotent)

### Phase 4 — True keyset internals (`feature-flag-cj5`) — DONE (Revision #2)
- [x] True keyset migration [feature-flag-cj5]: converted the 8 top-level list repos from `OFFSET` + `COUNT(*) OVER()` to keyset (`WHERE (created_at,id) > cursor … ORDER BY created_at, id LIMIT n+1`) via clean proto cutover (page/per_page/total → cursor/limit/next_cursor); opaque token owned by `stitchd_db::KeysetCursor`, forwarded by the gateway (`CursorPage::from_token`); interim encoded-offset shim removed. REST contract unchanged (opaque token) ⇒ Admin UI untouched. Each entity verified by a multi-page exactly-once/no-gaps test. Entities: flags [5827206], experiments [7e761c4], exclusion-groups [169342b], segments [4b4d454], events [1917f84], metrics [995469b], sdk-keys [418d251], org-users [599e36b] (+ keyset helper foundation [56aa12d], shim removal [28dc14e]).

## Phase 5: Fresh-DB Reset Tooling (feature-flag-7rp)
<!-- execution: parallel -->
<!-- depends: -->

- [x] Task 1: `scripts/reset_dev_db.sh` — drop + recreate + migrate from V1 baseline; non-interactive, idempotent; resolves baseline checksum drift [e604ccb]
  <!-- files: crates/xtask/src, scripts -->
- [x] Task 2: Document the fresh-DB verification flow (matches CI fresh-from-scratch) + close feature-flag-7rp [84bb55b]
  <!-- files: README.md, conductor/workflow.md -->
- [ ] Task: Conductor - User Manual Verification 'Fresh-DB Reset Tooling' (Protocol in workflow.md)
